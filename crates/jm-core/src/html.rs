//! Mail HTML: building draft bodies, finding the author's part again, and turning HTML into
//! something safe to show or readable as text.
//!
//! Drafts written by Just Mail carry two marked blocks so the author's text can be edited later
//! without touching the signature or the quoted mail below it:
//!
//! ```html
//! <div id="jm-body" style="…">text<br><br>more text</div>
//! <div id="jm-signature" style="…"><br>signature</div>
//! (quoted mail of a reply or forward)
//! ```
//!
//! Paragraphs are `<br><br>` inside one `<div>`, because `<p>` collapses in Outlook.

const FONT: &str = "font-family: Calibri, Helvetica, Arial, sans-serif; font-size: 11pt;";
const BODY_ID: &str = "jm-body";
const SIGNATURE_ID: &str = "jm-signature";

/// Pictures in the author's text are shown at most this many pixels wide.
const TEXT_PICTURE_W: u32 = 600;

/// Plain text to mail HTML: escaped, line breaks as `<br>`.
pub fn text_to_html(text: &str) -> String {
    text_to_html_with(text, &[])
}

/// A picture in the author's text: `[name]` in the editor, its `<img>` in the mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPicture {
    pub name: String,
    pub tag: String,
}

impl TextPicture {
    /// The picture attached as `content_id`, `size` pixels big (shown at most 600 wide).
    pub fn new(name: &str, content_id: &str, (width, height): (u32, u32)) -> Self {
        let (w, h) = if width > TEXT_PICTURE_W {
            (TEXT_PICTURE_W, (u64::from(height) * u64::from(TEXT_PICTURE_W) / u64::from(width)).max(1) as u32)
        } else {
            (width, height)
        };
        let alt = html_escape::encode_double_quoted_attribute(name);
        let tag = format!(
            "<img src=\"cid:{content_id}\" alt=\"{alt}\" width=\"{w}\" height=\"{h}\" style=\"max-width:100%;height:auto\">"
        );
        TextPicture { name: name.to_string(), tag }
    }

    /// How the editor shows it.
    pub fn marker(&self) -> String {
        format!("[{}]", self.name)
    }

    pub fn content_id(&self) -> Option<String> {
        content_ids(&self.tag).into_iter().next()
    }
}

/// [`text_to_html`] with every `[name]` of one of `pictures` as that picture.
pub fn text_to_html_with(text: &str, pictures: &[TextPicture]) -> String {
    let markers: Vec<(String, &str)> =
        pictures.iter().map(|p| (html_escape::encode_text(&p.marker()).into_owned(), p.tag.as_str())).collect();
    let text = text.replace("\r\n", "\n");
    text.trim_end_matches('\n')
        .split('\n')
        .map(|line| {
            // keep indentation visible
            let indent = line.len() - line.trim_start_matches(' ').len();
            let mut line = html_escape::encode_text(&line[indent..]).into_owned();
            for (marker, tag) in &markers {
                line = line.replace(marker, tag);
            }
            format!("{}{line}", "&nbsp;".repeat(indent))
        })
        .collect::<Vec<_>>()
        .join("<br>\n")
}

/// [`html_to_text`] with each embedded picture (`<img src="cid:…">`) as `[name]`, its `alt` text
/// or else its content id, and those pictures.
pub fn html_to_text_with_pictures(html: &str) -> (String, Vec<TextPicture>) {
    let lower = html.to_ascii_lowercase();
    let mut marked = String::with_capacity(html.len());
    let mut pictures: Vec<TextPicture> = Vec::new();
    let mut copied = 0;
    for (start, end) in img_tags(&lower) {
        let Some((s, e)) = attr_value(&lower, start, end, "src") else { continue };
        if !lower[s..e].trim().starts_with("cid:") {
            continue;
        }
        let alt = attr_value(&lower, start, end, "alt").map(|(s, e)| html_escape::decode_html_entities(&html[s..e]).trim().to_string());
        let stem = alt.filter(|a| !a.is_empty()).unwrap_or_else(|| {
            let id = content_id(&html[s..e].trim()[4..]);
            id.split('@').next().unwrap_or_default().to_string()
        });
        // a name in the text stands for one picture
        let mut name = stem.clone();
        let mut n = 1;
        while pictures.iter().any(|p| p.name == name) {
            n += 1;
            name = format!("{stem} ({n})");
        }
        let tag_end = (end + 1).min(html.len());
        marked.push_str(&html[copied..start]);
        marked.push_str(&html_escape::encode_text(&format!("[{name}]")));
        copied = tag_end;
        pictures.push(TextPicture { name, tag: html[start..tag_end].to_string() });
    }
    marked.push_str(&html[copied..]);
    (html_to_text(&marked), pictures)
}

/// A token that keeps the content ids of one draft's inline images apart from any other's
/// (`…@jm-…`).
pub fn content_id_token() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("jm-{nanos:x}")
}

/// Whether Just Mail gave the inline image this content id (a picture or signature logo).
pub fn is_own_content_id(content_id: &str) -> bool {
    content_id.contains("@jm-")
}

/// The marked author block followed by the signature block (if `signature_html` isn't blank,
/// see [`crate::AccountConfig::signature_block`]).
pub fn compose_blocks(body_html: &str, signature_html: &str) -> String {
    let mut out = format!("<div id=\"{BODY_ID}\" style=\"{FONT}\">{body_html}</div>\n");
    if !signature_html.trim().is_empty() {
        out.push_str(&format!("<div id=\"{SIGNATURE_ID}\" style=\"{FONT}\"><br>\n{}</div>\n", signature_html.trim()));
    }
    out
}

/// A complete HTML document for a new mail.
pub fn new_document(body_html: &str, signature_html: &str) -> String {
    format!("<html><body>\n{}</body></html>", compose_blocks(body_html, signature_html))
}

/// An image taken out of the HTML for an inline attachment (`<img src="cid:…">`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub content_id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Turn every `<img src="data:image/…;base64,…">` into a `cid:` reference and return the images
/// as inline attachments to add (Outlook blocks `data:` images). Files are named
/// `{stem}-1.png`, `{stem}-2.jpg`, …; content ids are `{name}@{token}`, so a token unique per
/// draft keeps them apart from the ids a quoted mail brings along. Images that don't decode stay.
pub fn inline_data_images(html: &str, stem: &str, token: &str) -> (String, Vec<InlineImage>) {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut copied = 0;
    for (tag_start, tag_end) in img_tags(&lower) {
        let Some((start, end)) = attr_value(&lower, tag_start, tag_end, "src") else { continue };
        let value = &html[start..end];
        let Some((content_type, data)) = value
            .strip_prefix("data:")
            .and_then(|v| v.split_once(','))
            .and_then(|(meta, data)| Some((meta.strip_suffix(";base64")?, data)))
            .filter(|(meta, _)| meta.to_ascii_lowercase().starts_with("image/"))
        else {
            continue;
        };
        let Some(bytes) = decode_base64(data) else { continue };
        let content_type = content_type.to_ascii_lowercase();
        let ext = match content_type.trim_start_matches("image/") {
            "jpeg" | "pjpeg" => "jpg",
            "svg+xml" => "svg",
            other => other,
        };
        let name = format!("{stem}-{}.{ext}", images.len() + 1);
        let content_id = format!("{name}@{token}");
        out.push_str(&html[copied..start]);
        out.push_str(&format!("cid:{content_id}"));
        copied = end;
        images.push(InlineImage { content_id, name, content_type, bytes });
    }
    out.push_str(&html[copied..]);
    (out, images)
}

/// Base64 as mail writes it: wrapped, padded or not.
fn decode_base64(data: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(data.trim_end_matches('=')).ok()
}

/// `(start, end)` of every `<img …>` tag in lowercased HTML, `end` at its `>`.
fn img_tags(lower: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut search = 0;
    std::iter::from_fn(move || {
        let start = search + lower.get(search..)?.find("<img")?;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        search = end;
        Some((start, end))
    })
}

/// Byte range of the value of attribute `name` inside the tag `lower[tag_start..tag_end]`.
fn attr_value(lower: &str, tag_start: usize, tag_end: usize, name: &str) -> Option<(usize, usize)> {
    let tag = &lower[tag_start..tag_end];
    let mut from = 0;
    let attr = loop {
        let at = from + tag[from..].find(name)?;
        let before = tag[..at].chars().next_back();
        from = at + name.len();
        if before.is_some_and(char::is_whitespace) && tag[from..].trim_start().starts_with('=') {
            break at;
        }
    };
    let after_eq = attr + name.len() + tag[attr + name.len()..].find('=')? + 1;
    let rest = &tag[after_eq..];
    let value_start = after_eq + (rest.len() - rest.trim_start().len());
    let (start, end) = match tag[value_start..].chars().next()? {
        quote @ ('"' | '\'') => {
            let start = value_start + 1;
            (start, start + tag[start..].find(quote)?)
        }
        _ => {
            let len = tag[value_start..].find(|c: char| c.is_whitespace() || c == '>').unwrap_or(tag.len() - value_start);
            (value_start, value_start + len)
        }
    };
    Some((tag_start + start, tag_start + end))
}

/// Put `blocks` at the very top of an existing document (above a reply's quote).
pub fn insert_at_top(document: &str, blocks: &str) -> String {
    match find_tag_end(document, "<body") {
        Some(end) => format!("{}\n{}{}", &document[..end], blocks, &document[end..]),
        None => format!("{blocks}<br>\n{document}"),
    }
}

/// Byte offset right after the `>` of the first `tag` (case-insensitive), e.g. `<body …>`.
fn find_tag_end(html: &str, tag: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find(tag)?;
    // make sure we matched `<body` and not `<bodyfoo`
    let next = lower[start + tag.len()..].chars().next()?;
    if !(next == '>' || next.is_whitespace()) {
        return None;
    }
    lower[start..].find('>').map(|i| start + i + 1)
}

/// Inner HTML range of the element with `id`. Exchange may rewrite quotes or add an `x_`
/// prefix, so both spellings are tried.
fn element_inner(html: &str, id: &str) -> Option<(usize, usize)> {
    let lower = html.to_ascii_lowercase();
    let needles =
        [format!("id=\"{id}\""), format!("id={id}"), format!("id='{id}'"), format!("id=\"x_{id}\"")];
    let attr = needles.iter().find_map(|n| lower.find(n.as_str()))?;
    let open_start = lower[..attr].rfind('<')?;
    let tag: String = lower[open_start + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if tag.is_empty() {
        return None;
    }
    let inner_start = open_start + lower[open_start..].find('>')? + 1;
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut depth = 1;
    let mut pos = inner_start;
    while depth > 0 {
        let next_open = lower[pos..].find(&open).map(|i| pos + i);
        let next_close = lower[pos..].find(&close).map(|i| pos + i)?;
        match next_open {
            Some(o) if o < next_close => {
                depth += 1;
                pos = o + open.len();
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    return Some((inner_start, next_close));
                }
                pos = next_close + close.len();
            }
        }
    }
    None
}

/// The author's part of a draft as HTML, if the draft has Just Mail's markers.
pub fn author_html(document: &str) -> Option<&str> {
    element_inner(document, BODY_ID).map(|(s, e)| &document[s..e])
}

/// Replace the author's part of a draft, leaving signature and quote as they are. `None` when
/// the draft has no Just Mail markers (the caller then builds a whole new document).
pub fn replace_author_html(document: &str, body_html: &str) -> Option<String> {
    element_inner(document, BODY_ID).map(|(s, e)| format!("{}{}{}", &document[..s], body_html, &document[e..]))
}

/// Whether the draft already carries a signature block.
pub fn has_signature(document: &str) -> bool {
    element_inner(document, SIGNATURE_ID).is_some()
}

/// The quoted mail of a reply or forward draft: what follows the signature block (or, without
/// one, the author's block), up to `</body>`. `None` for a new mail or a draft without markers.
pub fn quote_html(document: &str) -> Option<&str> {
    let (_, inner_end) = element_inner(document, SIGNATURE_ID).or_else(|| element_inner(document, BODY_ID))?;
    let after = document[inner_end..].find('>').map(|i| inner_end + i + 1)?;
    let rest = &document[after..];
    // ASCII lowercasing keeps byte offsets
    let rest = match rest.to_ascii_lowercase().rfind("</body") {
        Some(end) => &rest[..end],
        None => rest,
    };
    (!html_to_text(rest).trim().is_empty()).then_some(rest)
}

/// HTML to editable plain text: the inverse of [`text_to_html`] for our own markup, a decent
/// approximation for anything else.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        push_collapsed(&mut out, &rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            rest = &rest[lt..];
            break;
        };
        let tag = rest[lt + 1..lt + gt].trim().to_ascii_lowercase();
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let closing = tag.starts_with('/');
        match name.as_str() {
            "br" => out.push('\n'),
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
                if closing && !out.ends_with('\n') =>
            {
                out.push('\n')
            }
            "style" | "script" | "head" | "title" if !closing => {
                // skip to the matching closing tag
                let after = &rest[lt + gt + 1..];
                let close = format!("</{name}");
                match after.to_ascii_lowercase().find(&close) {
                    Some(i) => {
                        rest = &after[i..];
                        continue;
                    }
                    None => {
                        rest = "";
                        break;
                    }
                }
            }
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    push_collapsed(&mut out, rest);
    // source line breaks became single spaces at line starts; entities (&nbsp; indentation) are
    // still encoded here, so trimming plain spaces keeps intended indentation
    let lines: Vec<&str> = out.lines().map(|l| l.trim_start_matches(' ').trim_end()).collect();
    let text = html_escape::decode_html_entities(&lines.join("\n")).replace('\u{a0}', " ");
    let mut text = text.trim().to_string();
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text
}

/// Append text the way HTML shows it: runs of whitespace collapse into one space.
fn push_collapsed(out: &mut String, text: &str) {
    let mut last_space = out.ends_with(' ');
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
    }
}

/// Readable plain text of a whole mail, links as footnotes (for the CLI and MCP). What the mail's
/// CSS hides (a newsletter's preview text) stays out.
pub fn to_readable_text(html: &str, width: usize) -> String {
    html2text::config::plain().use_doc_css().string_from_read(html.as_bytes(), width).unwrap_or_else(|_| html_to_text(html))
}

/// Clean mail HTML for display: no scripts, styles or event handlers, nothing the mail hides,
/// no remote images unless asked for (they are mostly tracking pixels), layout tables flattened.
pub fn sanitize_for_display(html: &str, remote_images: bool) -> String {
    // a first pass keeps `style` and normalizes the markup, so hidden elements can be cut out
    let mut styled = ammonia::Builder::default();
    styled.add_generic_attributes(["style"]);
    let html = drop_hidden(&styled.clean(html).to_string());
    let mut builder = ammonia::Builder::default();
    builder.link_rel(Some("noopener noreferrer"));
    if !remote_images {
        builder.rm_tags(["img"]);
    }
    flatten_layout_tables(&join_short_rows(&builder.clean(&html).to_string()))
}

/// The HTML without the elements its inline CSS hides, with all they contain: the preview text
/// a newsletter puts first for the inbox list (`display: none`) mostly. Expects normalized HTML
/// (ammonia output with `style` kept).
fn drop_hidden(html: &str) -> String {
    const VOID: [&str; 6] = ["br", "hr", "img", "input", "meta", "wbr"];
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        let tail = &rest[lt + 1..];
        let name: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let hidden = tail.find('>').filter(|_| !name.is_empty()).filter(|&gt| {
            let lower = tail[..gt].to_ascii_lowercase();
            attr_value(&lower, 0, gt, "style").is_some_and(|(s, e)| hides(&lower[s..e]))
        });
        let Some(gt) = hidden else {
            out.push_str(&rest[..lt + 1]);
            rest = tail;
            continue;
        };
        out.push_str(&rest[..lt]);
        rest = &tail[gt + 1..];
        if !VOID.contains(&name.as_str()) {
            // an end tag that never comes keeps the content: better shown than lost
            rest = skip_element(rest, &name).unwrap_or(rest);
        }
    }
    out.push_str(rest);
    out
}

/// What follows the end tag of the `name` element whose content starts `html`.
fn skip_element<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let (open, close) = (format!("<{name}"), format!("</{name}>"));
    let mut depth = 1;
    let mut pos = 0;
    while depth > 0 {
        let next_close = pos + html[pos..].find(&close)?;
        // `<b` must not count a `<br>`
        let next_open = html[pos..next_close]
            .match_indices(&open)
            .map(|(i, _)| pos + i)
            .find(|&i| matches!(html.as_bytes().get(i + open.len()), Some(b'>' | b' ' | b'/')));
        match next_open {
            Some(o) => {
                depth += 1;
                pos = o + open.len();
            }
            None => {
                depth -= 1;
                pos = next_close + close.len();
            }
        }
    }
    Some(&html[pos..])
}

/// Whether inline CSS hides its element: `display: none`, `visibility: hidden`, or no height
/// with the overflow cut off.
fn hides(style: &str) -> bool {
    let decls: Vec<(&str, &str)> = style
        .split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim().trim_end_matches("!important").trim()))
        .collect();
    let has = |key: &str, value: &dyn Fn(&str) -> bool| decls.iter().any(|(k, v)| *k == key && value(v));
    let zero = |v: &str| v.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<f32>().is_ok_and(|n| n == 0.);
    has("display", &|v| v == "none")
        || has("visibility", &|v| v == "hidden")
        || (has("max-height", &zero) && has("overflow", &|v| v == "hidden"))
}

/// Mail HTML as Markdown for display: cleaned like [`sanitize_for_display`], then converted, so
/// paragraphs, line breaks, lists, emphasis and links survive while layout noise goes. Pictures
/// show when they can: the embedded ones (`images` for `cid:` references, `data:` ones) and,
/// with `remote_images`, those from the web that say how big they are.
pub fn to_display_markdown(html: &str, remote_images: bool, images: &[InlineImage]) -> String {
    let (html, pictures) = take_pictures(html, images, remote_images);
    let clean = sanitize_for_display(&html, remote_images);
    let converter = htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options { br_style: htmd::options::BrStyle::Backslash, ..Default::default() })
        .build();
    let md = converter.convert(&clean).unwrap_or_else(|_| html_to_text(&clean));
    // a line holding only a hard break or nothing at all is spacing; keep at most one
    let lines: Vec<&str> = md.lines().map(str::trim_end).collect();
    let is_space = |l: &str| {
        let l = l.trim();
        l.is_empty() || l == "\\"
    };
    let mut out = String::with_capacity(md.len());
    let mut blank = 0;
    for (i, line) in lines.iter().enumerate() {
        if is_space(line) {
            blank += 1;
            if blank == 1 {
                out.push('\n');
            }
            continue;
        }
        blank = 0;
        // a hard break right before a paragraph end would show as a literal backslash
        let ends_paragraph = lines.get(i + 1).is_none_or(|next| is_space(next));
        let line = if ends_paragraph { line.trim_end_matches('\\').trim_end() } else { line };
        out.push_str(line);
        out.push('\n');
    }
    let mut out = unwrap_picture_links(out.trim());
    for (n, picture) in pictures.iter().enumerate() {
        out = out.replace(&format!("{PICTURE}{n}Z"), picture);
    }
    out
}

/// Mail HTML as a page of its own for the app's web view, which shows it as designed: embedded
/// pictures (`cid:` in `images`) become `data:` URLs, and what could run, navigate or load on its
/// own goes (scripts, frames, plugins, a refresh, a base URL). The web view adds the rest: a
/// sandbox without scripts and a content policy that keeps remote content out unless allowed.
pub fn to_web_document(html: &str, images: &[InlineImage]) -> String {
    use base64::Engine as _;
    let html = strip_active(html);
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    for (start, end) in img_tags(&lower) {
        let Some((s, e)) = attr_value(&lower, start, end, "src") else { continue };
        let Some(id) = lower[s..e].trim().strip_prefix("cid:").map(|_| content_id(&html[s..e].trim()[4..])) else { continue };
        let Some(image) = images.iter().find(|i| i.content_id.eq_ignore_ascii_case(&id)) else { continue };
        let mime = image_type(&image.bytes).unwrap_or(image.content_type.as_str());
        out.push_str(&html[copied..s]);
        out.push_str(&format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&image.bytes)));
        copied = e;
    }
    out.push_str(&html[copied..]);
    out
}

/// The HTML without scripts, frames, plugins, refreshes and base URLs. Works on raw mail HTML.
fn strip_active(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let (mut copied, mut pos) = (0, 0);
    while let Some(lt) = lower[pos..].find('<').map(|i| pos + i) {
        let name: String = lower[lt + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let Some(gt) = lower[lt..].find('>').map(|i| lt + i) else { break };
        let end = match name.as_str() {
            // their content is raw text up to the first end tag
            "script" | "iframe" | "noembed" | "noframes" => {
                let close = format!("</{name}");
                lower[gt..].find(&close).and_then(|i| lower[gt + i..].find('>').map(|j| gt + i + j + 1)).unwrap_or(lower.len())
            }
            "object" | "embed" | "applet" | "frame" | "frameset" | "base" => gt + 1,
            "meta" if lower[lt..gt].contains("refresh") => gt + 1,
            _ => {
                pos = gt + 1;
                continue;
            }
        };
        out.push_str(&html[copied..lt]);
        (copied, pos) = (end, end);
    }
    out.push_str(&html[copied..]);
    out
}

/// Whether the mail brings its own background colors (a newsletter, a designed signature). Such
/// mail is shown as designed on light even in dark mode; plain mail takes the app's colors.
pub fn has_own_colors(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let set = |value: &str| {
        let value = value.trim_start_matches(|c: char| c == '=' || c == ':' || c == '"' || c == '\'' || c.is_whitespace());
        let value = value.split([';', '"', '\'', '}', '>']).next().unwrap_or("").trim();
        !matches!(value, "" | "none" | "transparent" | "inherit" | "initial" | "unset")
    };
    lower.match_indices("bgcolor").any(|(i, _)| set(&lower[i + 7..]))
        || lower.match_indices("background").any(|(i, _)| {
            let rest = &lower[i + 10..];
            let rest = rest.strip_prefix("-color").or_else(|| rest.strip_prefix("-image")).unwrap_or(rest);
            rest.trim_start().starts_with([':', '=']) && set(rest)
        })
}

/// Pictures wider than this many pixels are shown scaled down.
const MAX_PICTURE_W: u32 = 560;
/// Stands in for a picture while the HTML is cleaned and converted, as `JMPICTURE{n}Z`: letters
/// and digits only, so nothing on the way escapes or drops it.
const PICTURE: &str = "JMPICTURE";

/// Content ids of the pictures the HTML embeds (`<img src="cid:…">`), to fetch them.
pub fn content_ids(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut ids = Vec::new();
    for (start, end) in img_tags(&lower) {
        let Some((s, e)) = attr_value(&lower, start, end, "src") else { continue };
        if let Some(id) = html[s..e].trim().get(4..).filter(|_| lower[s..e].trim().starts_with("cid:")) {
            let id = content_id(id);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// A content id as attachments carry it: without `<>`, `%40` back to `@`.
pub fn content_id(raw: &str) -> String {
    raw.trim().trim_start_matches('<').trim_end_matches('>').replace("%40", "@")
}

/// Take out the pictures that can be shown and put a placeholder word in their place: embedded
/// ones (`cid:` found in `images`, `data:`) always, remote ones when allowed. Each needs a size,
/// from its attributes or its bytes; ones without stay for the sanitizer, tracking pixels go.
/// Returns the HTML and, per placeholder, the `<img>` to put back after conversion.
fn take_pictures(html: &str, images: &[InlineImage], remote: bool) -> (String, Vec<String>) {
    use base64::Engine as _;
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pictures = Vec::new();
    let mut copied = 0;
    for (start, end) in img_tags(&lower) {
        let Some((s, e)) = attr_value(&lower, start, end, "src") else { continue };
        let (value, kind) = (html[s..e].trim(), lower[s..e].trim());
        let (src, natural) = if kind.starts_with("cid:") {
            let id = content_id(&value[4..]);
            let image = images.iter().find(|i| i.content_id.eq_ignore_ascii_case(&id));
            let Some((image, mime)) = image.and_then(|i| Some((i, image_type(&i.bytes)?))) else {
                // not fetched, or not a picture: nothing could show it
                out.push_str(&html[copied..start]);
                copied = (end + 1).min(html.len());
                continue;
            };
            let data = base64::engine::general_purpose::STANDARD.encode(&image.bytes);
            (format!("data:{mime};base64,{data}"), image_size(&image.bytes))
        } else if kind.starts_with("data:image/") {
            let bytes = value.split_once(',').and_then(|(_, data)| decode_base64(data));
            (value.chars().filter(|c| !c.is_whitespace()).collect(), bytes.and_then(|b| image_size(&b)))
        } else if remote && (kind.starts_with("https://") || kind.starts_with("http://")) {
            (value.to_string(), None)
        } else {
            continue;
        };
        let attr = |name| {
            let (s, e) = attr_value(&lower, start, end, name)?;
            lower[s..e].trim().trim_end_matches("px").parse::<u32>().ok().filter(|v| *v > 0)
        };
        let size = match (attr("width"), attr("height"), natural) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some((nw, nh))) => (w, w * nh / nw),
            (None, Some(h), Some((nw, nh))) => (h * nw / nh, h),
            (None, None, Some(natural)) => natural,
            _ => continue,
        };
        if src.contains('"') {
            continue;
        }
        out.push_str(&html[copied..start]);
        copied = (end + 1).min(html.len());
        let (w, h) = size;
        if w <= 2 || h <= 2 {
            continue;
        }
        let (w, h) = if w > MAX_PICTURE_W { (MAX_PICTURE_W, (h * MAX_PICTURE_W / w).max(1)) } else { (w, h) };
        out.push_str(&format!("{PICTURE}{}Z", pictures.len()));
        pictures.push(format!("<img src=\"{src}\" width=\"{w}\" height=\"{h}\">"));
    }
    out.push_str(&html[copied..]);
    (out, pictures)
}

/// Links in converted Markdown whose text is only space go (a linked picture that could not be
/// shown leaves one behind, an underlined blank); a link around just a picture becomes the
/// picture, so it shows as one.
fn unwrap_picture_links(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let link = after.find("](").and_then(|mid| {
            let text = &after[..mid];
            let close = mid + 2 + after[mid + 2..].find(')')?;
            (!text.contains(['[', ']', '\n'])).then_some((text, close))
        });
        // `![alt](src)` is an image, not a link
        let (Some((text, close)), false) = (link, rest[..open].ends_with('!')) else {
            out.push_str(&rest[..open + 1]);
            rest = after;
            continue;
        };
        let bare = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{a0}');
        let is_picture = bare.strip_prefix(PICTURE).and_then(|n| n.strip_suffix('Z')).is_some_and(|n| n.parse::<usize>().is_ok());
        if bare.is_empty() || is_picture {
            out.push_str(&rest[..open]);
            out.push_str(text);
        } else {
            out.push_str(&rest[..open + 1 + close + 1]);
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// MIME type of a picture, from its first bytes.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// Width and height of a PNG, GIF or JPEG, from its header.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes(bytes.get(i..i + 2)?.try_into().ok()?) as u32);
    let size = match image_type(bytes)? {
        "image/png" => {
            let be32 = |i: usize| Some(u32::from_be_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
            (be32(16)?, be32(20)?)
        }
        "image/gif" => {
            let le16 = |i: usize| Some(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?) as u32);
            (le16(6)?, le16(8)?)
        }
        "image/jpeg" => {
            // walk the segments up to a start-of-frame marker, which holds height and width
            let mut i = 2;
            loop {
                let (&0xFF, &marker) = (bytes.get(i)?, bytes.get(i + 1)?) else { return None };
                if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    break (be16(i + 7)?, be16(i + 5)?);
                }
                i += 2 + be16(i + 2)? as usize;
            }
        }
        _ => return None,
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Table rows of short cells side by side (a signature's "Phone | +41 …", a footer's links) stay
/// one line: their cells are joined with a wide space, the paragraphs inside them dropped. Rows
/// with a long text, a line break or a block in a cell are left to [`flatten_layout_tables`].
/// Expects normalized HTML (ammonia output).
fn join_short_rows(html: &str) -> String {
    const BLOCKS: [&str; 10] = ["<br", "<table", "<ul", "<ol", "<li", "<h1", "<h2", "<h3", "<h4", "<blockquote"];
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(close) = html[cursor..].find("</tr>").map(|i| cursor + i) {
        let end = close + "</tr>".len();
        // the innermost row: the last `<tr` before its end
        let row = html[cursor..close].rfind("<tr").map(|i| cursor + i).filter(|&s| {
            matches!(html.as_bytes().get(s + 3), Some(b'>' | b' ')) && !html[s..close].contains("<table")
        });
        let Some(start) = row else {
            out.push_str(&html[cursor..end]);
            cursor = end;
            continue;
        };
        let cells: Vec<&str> = cells(&html[start..close]).into_iter().filter(|c| !html_to_text(c).is_empty() || c.contains(PICTURE)).collect();
        let short = cells.len() >= 2
            && cells.iter().all(|c| html_to_text(c).chars().count() <= 60 && !BLOCKS.iter().any(|b| c.contains(b)));
        out.push_str(&html[cursor..start]);
        if short {
            let joined: Vec<String> = cells.iter().map(|c| drop_tags(c, &["p", "div"])).collect();
            out.push_str(&format!("<tr><td>{}</td></tr>", joined.join("\u{2003}")));
        } else {
            out.push_str(&html[start..end]);
        }
        cursor = end;
    }
    out.push_str(&html[cursor..]);
    out
}

/// The inner HTML of each `<td>`/`<th>` in a row without nested tables.
fn cells(row: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = row;
    while let Some(open) = [rest.find("<td"), rest.find("<th")].into_iter().flatten().min() {
        let Some(inner) = rest[open..].find('>').map(|i| open + i + 1) else { break };
        let close = rest[inner..].find("</t").map_or(rest.len(), |i| inner + i);
        out.push(&rest[inner..close]);
        rest = rest.get(close + 3..).unwrap_or("");
    }
    out
}

/// The HTML without the tags called `names` (their content stays).
fn drop_tags(html: &str, names: &[&str]) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tail = &rest[lt + 1..];
        let name: String = tail.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        match tail.find('>') {
            Some(gt) if names.contains(&name.as_str()) => rest = &tail[gt + 1..],
            _ => {
                out.push('<');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Most tables in mail are layout scaffolding (newsletters nest them five deep). Shown as tables
/// they squeeze whole paragraphs into one cell, so table markup is dropped: the paragraphs inside
/// become siblings (and get paragraph spacing), and a cell that ends in bare text ends with a
/// line break. `<div>`s get the same treatment: Outlook and Spark write one `<div>` per line
/// (`<div><br></div>` for an empty one), so a div is a line, not a paragraph. Expects normalized
/// HTML (ammonia output: lowercase tags, quoted attributes).
fn flatten_layout_tables(html: &str) -> String {
    const DROP: [&str; 10] = ["table", "tbody", "thead", "tfoot", "tr", "td", "th", "div", "center", "caption"];
    const BLOCK_ENDS: [&str; 12] =
        ["</p>", "</div>", "</ul>", "</ol>", "</li>", "</h1>", "</h2>", "</h3>", "</h4>", "</blockquote>", "<br>", "<hr>"];
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tail = &rest[lt + 1..];
        let closing = tail.starts_with('/');
        let name_start = if closing { 1 } else { 0 };
        let name: String = tail[name_start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        match tail.find('>') {
            Some(gt) if DROP.contains(&name.as_str()) => {
                if closing && (name == "td" || name == "th" || name == "div") {
                    let end = out.trim_end();
                    if !end.is_empty() && !BLOCK_ENDS.iter().any(|b| end.ends_with(b)) {
                        out.push_str("<br>");
                    }
                }
                rest = &tail[gt + 1..];
            }
            _ => {
                out.push('<');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether the HTML references remote images (to offer a "load images" button).
pub fn has_remote_images(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.match_indices("<img").any(|(i, _)| {
        let tag_end = lower[i..].find('>').map(|e| i + e).unwrap_or(lower.len());
        let tag = &lower[i..tag_end];
        tag.contains("src=\"http") || tag.contains("src='http") || tag.contains("src=http")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrips_through_our_html() {
        let text = "Hallo Martin,\n\nzwei Zeilen\n  eingerückt & <spitz>\n\nGrüße";
        let html = text_to_html(text);
        assert!(html.contains("&lt;spitz&gt;"));
        assert_eq!(html_to_text(&html), text);
    }

    #[test]
    fn pictures_roundtrip_through_the_text() {
        let picture = TextPicture::new("Bild 1.png", "picture.png@jm-1", (1200, 801));
        assert_eq!(picture.content_id().as_deref(), Some("picture.png@jm-1"));
        assert!(picture.tag.contains("width=\"600\" height=\"400\""));
        let text = "Hallo,\n\nhier:\n[Bild 1.png]\n\n[Bild 2.png] ist keins";
        let html = text_to_html_with(text, std::slice::from_ref(&picture));
        assert!(html.contains("hier:<br>\n<img src=\"cid:picture.png@jm-1\" alt=\"Bild 1.png\""));
        assert!(html.contains("[Bild 2.png] ist keins"));
        let (back, pictures) = html_to_text_with_pictures(&html);
        assert_eq!(back, text);
        assert_eq!(pictures, vec![picture]);
        assert!(is_own_content_id("picture.png@jm-1"));

        // without alt text the content id names it; a name stands for one picture only
        let (text, pictures) =
            html_to_text_with_pictures("a<br><img src=\"cid:shot.png@x\"><br><IMG ALT=\"shot.png\" SRC=\"cid:b@x\"><img src=\"logo.png\">");
        assert_eq!(text, "a\n[shot.png]\n[shot.png (2)]");
        assert_eq!(pictures[1].tag, "<IMG ALT=\"shot.png\" SRC=\"cid:b@x\">");
        assert_eq!(text_to_html_with(&text, &pictures), "a<br>\n<img src=\"cid:shot.png@x\"><br>\n<IMG ALT=\"shot.png\" SRC=\"cid:b@x\">");
    }

    #[test]
    fn finds_and_replaces_the_author_block() {
        let doc = new_document("alt<br>\n<div>nested</div>", "Marc");
        assert_eq!(author_html(&doc), Some("alt<br>\n<div>nested</div>"));
        assert!(has_signature(&doc));
        let changed = replace_author_html(&doc, "neu").unwrap();
        assert_eq!(author_html(&changed), Some("neu"));
        assert_eq!(changed.matches("id=\"jm-signature\"").count(), 1);
        assert_eq!(replace_author_html("<p>from Outlook</p>", "neu"), None);
    }

    #[test]
    fn data_images_become_inline_attachments() {
        // "PNG" and "JFIF" as base64, the second one wrapped and unpadded
        let html = "<div>Gruß<br><IMG alt=\"logo\" SRC=\"data:image/png;base64,UE5H\"><br>\
                    <img data-src=\"x\" src='data:image/jpeg;base64,SkZ\n JRg'>\
                    <img src=\"https://example.com/remote.png\"><img src=\"data:image/png;base64,!!!\"></div>";
        let (out, images) = inline_data_images(html, "signature", "jm-1a2b");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].name, "signature-1.png");
        assert_eq!(images[0].content_id, "signature-1.png@jm-1a2b");
        assert_eq!((images[0].content_type.as_str(), images[0].bytes.as_slice()), ("image/png", b"PNG".as_slice()));
        assert_eq!((images[1].name.as_str(), images[1].bytes.as_slice()), ("signature-2.jpg", b"JFIF".as_slice()));
        assert!(out.contains("<IMG alt=\"logo\" SRC=\"cid:signature-1.png@jm-1a2b\">"), "{out}");
        assert!(out.contains("data-src=\"x\" src='cid:signature-2.jpg@jm-1a2b'"), "{out}");
        // remote and broken images stay as they were
        assert!(out.contains("https://example.com/remote.png") && out.contains("base64,!!!"));
        assert_eq!(inline_data_images("<p>no images</p>", "s", "t"), ("<p>no images</p>".to_string(), vec![]));
    }

    #[test]
    fn signature_html_goes_into_its_own_block() {
        let doc = new_document("Hallo", "<div>Herzliche Grüße<br>Marc</div>");
        assert!(doc.contains("<div id=\"jm-signature\" style=\""));
        assert!(doc.contains("<br>\n<div>Herzliche Grüße<br>Marc</div></div>"));
        assert!(!new_document("Hallo", "  ").contains("jm-signature"));
    }

    #[test]
    fn finds_blocks_after_exchange_rewrote_them() {
        let doc = "<html><head></head><body><div id=\"x_jm-body\" style=\"a\">hi</div><div>quote</div></body></html>";
        assert_eq!(author_html(doc), Some("hi"));
    }

    #[test]
    fn finds_the_quote_below_the_signature() {
        let new = new_document("Hallo", "<div>Marc</div>");
        assert_eq!(quote_html(&new), None);
        let reply = insert_at_top("<html><body><hr><div>Von: Tom</div><div>alte Mail</div></body></html>", &compose_blocks("Danke", "<div>Marc</div>"));
        let quote = quote_html(&reply).unwrap();
        assert!(quote.contains("alte Mail") && !quote.contains("Danke") && !quote.contains("Marc"));
    }

    #[test]
    fn inserts_above_a_quote() {
        let doc = "<html><head><meta charset=\"utf-8\"></head><body dir=\"ltr\"><hr>quote</body></html>";
        let out = insert_at_top(doc, &compose_blocks("answer", ""));
        let body = out.find("<body").unwrap();
        assert!(out.find("jm-body").unwrap() > body && out.find("jm-body").unwrap() < out.find("<hr>").unwrap());
    }

    #[test]
    fn drops_remote_images_unless_asked() {
        let html = "<p>x<img src=\"https://t.example/p.gif\"></p><script>alert(1)</script>";
        assert!(has_remote_images(html));
        let clean = sanitize_for_display(html, false);
        assert!(!clean.contains("<img") && !clean.contains("script"));
        assert!(sanitize_for_display(html, true).contains("<img"));
    }

    #[test]
    fn hidden_preview_text_stays_out() {
        let html = "<body><div style=\"display:none; font-size:1px; max-height:0px; overflow:hidden\">Vorschau \
                    <div>verschachtelt</div><b>fett</b><br></div>\
                    <span style=\"max-height: 0; overflow: hidden\">auch weg</span>\
                    <p style=\"Display: None !important\">weg</p>\
                    <div style=\"font-size:0px\"><div style=\"font-size:16px\"><p>Hallo Anna,<br>sichtbar</p></div></div>\
                    <span style=\"max-height:0px\">bleibt</span><b style=\"display:none\">x</b><br>Ende</body>";
        let md = to_display_markdown(html, false, &[]);
        assert_eq!(md, "Hallo Anna,\\\nsichtbar\n\nbleibt\\\nEnde");
        let text = to_readable_text(html, 80);
        assert!(text.contains("Hallo Anna") && !text.contains("Vorschau") && !text.contains("weg"), "{text}");
    }

    #[test]
    fn web_document_inlines_pictures_and_drops_what_runs() {
        let logo = InlineImage {
            content_id: "logo@example".into(),
            name: "logo.png".into(),
            content_type: "application/octet-stream".into(),
            bytes: png(20, 10),
        };
        let html = "<html><head><META http-equiv=\"refresh\" content=\"0;url=https://x.example\"><base href=\"https://x.example/\">\
                    <SCRIPT>alert('<b>')</SCRIPT></head><body style=\"background:#f4f4f4\">\
                    <img src=\"cid:logo%40example\" width=\"20\"><img src=\"cid:missing\">\
                    <iframe src=\"https://x.example\">inner</iframe><object data=\"x.swf\">fallback</object>\
                    <p onclick=\"x()\">Text</p></body></html>";
        let doc = to_web_document(html, &[logo]);
        assert!(doc.contains("<img src=\"data:image/png;base64,iVBORw0KGgo"), "{doc}");
        assert!(doc.contains("<img src=\"cid:missing\">"));
        for gone in ["refresh", "<base", "alert", "SCRIPT", "iframe", "inner", "<object"] {
            assert!(!doc.contains(gone), "{gone} in {doc}");
        }
        assert!(doc.contains("fallback") && doc.contains("<p onclick=\"x()\">Text</p>"));
    }

    #[test]
    fn tells_designed_mail_from_plain() {
        assert!(has_own_colors("<body style=\"word-spacing:normal; background-color:#F4F4F4\">"));
        assert!(has_own_colors("<td bgcolor=\"#ffffff\">"));
        assert!(has_own_colors("<style>.x { background: url(x.png) }</style>"));
        assert!(!has_own_colors("<p style=\"color:#1F497D; background: transparent\">Gruß</p>"));
        assert!(!has_own_colors("<div style=\"background-repeat:no-repeat\">no background here</div>"));
    }

    #[test]
    fn layout_tables_become_blocks() {
        let html = "<table width=\"600\"><tr><td align=\"left\"><p>Hallo</p><p>Welt <b>fett</b></p></td></tr>\
                    <tr><td>Zeile</td><td>Zelle</td></tr></table>";
        let clean = sanitize_for_display(html, false);
        assert!(!clean.contains("<table") && !clean.contains("<td"));
        assert!(clean.contains("<p>Hallo</p><p>Welt <b>fett</b></p>"));
        assert!(clean.contains("Zeile\u{2003}Zelle<br>"), "{clean}");
        assert!(!clean.contains("</p><br>"));
    }

    #[test]
    fn display_markdown_keeps_paragraphs_breaks_and_links() {
        let html = "<table><tr><td><p>Hallo,</p><p>Text mit <a href=\"https://x.de\">Link</a> und <b>fett</b>.</p>\
                    <p>Danke!<br>Anna<br>Support</p></td></tr></table>";
        let md = to_display_markdown(html, false, &[]);
        assert!(md.contains("Hallo,\n\nText mit [Link](https://x.de) und **fett**."), "{md}");
        assert!(md.contains("Danke!\\\nAnna\\\nSupport"), "{md}");
        let trailing = to_display_markdown("<p>weiter.<br></p><p>Danke</p>", false, &[]);
        assert_eq!(trailing, "weiter.\n\nDanke");
        let indented = to_display_markdown("<div>This is a test<br>\n  <br></div>", false, &[]);
        assert_eq!(indented, "This is a test");
        // Outlook and Spark: a div per line, an empty div is an empty line
        let lines = "<div>Grüße<br>Marc</div><div>Roothirsch GmbH</div><div>Brockhäger Str. 188</div>\
                     <div><br></div><div>Neuer Absatz</div>";
        assert_eq!(
            to_display_markdown(lines, false, &[]),
            "Grüße\\\nMarc\\\nRoothirsch GmbH\\\nBrockhäger Str. 188\n\nNeuer Absatz"
        );
    }

    /// A PNG header saying `w` × `h`.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(w.to_be_bytes());
        bytes.extend(h.to_be_bytes());
        bytes
    }

    #[test]
    fn reads_picture_sizes() {
        assert_eq!(image_size(&png(452, 100)), Some((452, 100)));
        assert_eq!(image_size(b"GIF89a\x19\0\x17\0"), Some((25, 23)));
        // SOI, an APP0 segment of 4 bytes, then SOF0 with height 50 and width 226
        let jpeg = b"\xFF\xD8\xFF\xE0\0\x04ab\xFF\xC0\0\x11\x08\0\x32\0\xE2";
        assert_eq!((image_type(jpeg), image_size(jpeg)), (Some("image/jpeg"), Some((226, 50))));
        assert_eq!(image_size(b"not a picture"), None);
    }

    #[test]
    fn embedded_pictures_show_with_their_size() {
        let logo = InlineImage {
            content_id: "image001.png@01DD".into(),
            name: "image001.png".into(),
            content_type: "application/octet-stream".into(),
            bytes: png(452, 100),
        };
        let html = "<p><img width=\"226\" height=\"50\" src=\"cid:image001.png@01DD\" alt=\"Logo\"></p>\
                    <p><a href=\"https://linkedin.example\"><img src=\"cid:image001.png%4001DD\" width=\"25\"></a>\
                    <a href=\"https://gone.example\"><img src=\"cid:missing\" width=\"25\">&nbsp;</a></p>\
                    <p>Text<img src=\"https://t.example/p.gif\" width=\"1\" height=\"1\"></p>";
        let md = to_display_markdown(html, true, &[logo]);
        assert!(md.starts_with("<img src=\"data:image/png;base64,iVBORw0KGgo"), "{md}");
        assert!(md.contains("width=\"226\" height=\"50\">\n\n<img"), "{md}");
        // the linked one keeps its aspect ratio and loses the link; the missing one leaves nothing
        assert!(md.contains("width=\"25\" height=\"5\">"), "{md}");
        assert!(!md.contains("](") && !md.contains("gone.example") && !md.contains("t.example"), "{md}");
        // without the bytes nothing is shown, remote pictures not unless allowed
        assert!(!to_display_markdown(html, false, &[]).contains("<img"));
        assert_eq!(content_ids(html), ["image001.png@01DD", "missing"]);
    }

    #[test]
    fn short_table_rows_stay_one_line() {
        // an Outlook signature: a label and a value per row, each in its own paragraph
        let html = "<table><tr><td><p><b>Anna Muster</b><br>Projektmanager</p></td></tr>\
                    <tr><td><table><tr><td><p><b>Telefon</b></p></td><td><p>+1 555 0100</p></td></tr>\
                    <tr><td><p><b>Web</b></p></td><td><p><a href=\"https://x.example\">x.example</a></p></td></tr>\
                    </table></td></tr></table><p>Eine lange Zeile, die nicht zusammengezogen wird</p>";
        let md = to_display_markdown(html, false, &[]);
        assert!(md.contains("**Telefon**\u{2003}+1 555 0100\\\n**Web**\u{2003}[x.example](https://x.example)"), "{md}");
        assert!(md.contains("**Anna Muster**\\\nProjektmanager"), "{md}");
    }
}
