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

/// Plain text to mail HTML: escaped, line breaks as `<br>`.
pub fn text_to_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    text.trim_end_matches('\n')
        .split('\n')
        .map(|line| {
            // keep indentation visible
            let indent = line.len() - line.trim_start_matches(' ').len();
            format!("{}{}", "&nbsp;".repeat(indent), html_escape::encode_text(&line[indent..]))
        })
        .collect::<Vec<_>>()
        .join("<br>\n")
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
    use base64::Engine as _;
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut copied = 0;
    let mut search = 0;
    while let Some(i) = lower[search..].find("<img").map(|i| search + i) {
        let tag_end = lower[i..].find('>').map(|e| i + e).unwrap_or(lower.len());
        search = tag_end;
        let Some((start, end)) = src_value(&lower, i, tag_end) else { continue };
        let value = &html[start..end];
        let Some((content_type, data)) = value
            .strip_prefix("data:")
            .and_then(|v| v.split_once(','))
            .and_then(|(meta, data)| Some((meta.strip_suffix(";base64")?, data)))
            .filter(|(meta, _)| meta.to_ascii_lowercase().starts_with("image/"))
        else {
            continue;
        };
        let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        let Ok(bytes) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(data.trim_end_matches('=')) else {
            continue;
        };
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

/// Byte range of the `src` attribute value inside the tag `lower[tag_start..tag_end]`.
fn src_value(lower: &str, tag_start: usize, tag_end: usize) -> Option<(usize, usize)> {
    let tag = &lower[tag_start..tag_end];
    let mut from = 0;
    let attr = loop {
        let at = from + tag[from..].find("src")?;
        let before = tag[..at].chars().next_back();
        from = at + 3;
        if before.is_some_and(char::is_whitespace) && tag[from..].trim_start().starts_with('=') {
            break at;
        }
    };
    let after_eq = attr + 3 + tag[attr + 3..].find('=')? + 1;
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

/// Readable plain text of a whole mail, links as footnotes (for the CLI and MCP).
pub fn to_readable_text(html: &str, width: usize) -> String {
    html2text::from_read(html.as_bytes(), width).unwrap_or_else(|_| html_to_text(html))
}

/// Clean mail HTML for display: no scripts, styles or event handlers; remote images only on
/// request (they are mostly tracking pixels), embedded `cid:` images are dropped.
pub fn sanitize_for_display(html: &str, remote_images: bool) -> String {
    let mut builder = ammonia::Builder::default();
    builder.link_rel(Some("noopener noreferrer"));
    if !remote_images {
        builder.rm_tags(["img"]);
    }
    flatten_layout_tables(&builder.clean(html).to_string())
}

/// Mail HTML as Markdown for display: cleaned like [`sanitize_for_display`], then converted, so
/// paragraphs, line breaks, lists, emphasis and links survive while layout noise goes.
pub fn to_display_markdown(html: &str, remote_images: bool) -> String {
    let clean = sanitize_for_display(html, remote_images);
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
    out.trim().to_string()
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
    fn layout_tables_become_blocks() {
        let html = "<table width=\"600\"><tr><td align=\"left\"><p>Hallo</p><p>Welt <b>fett</b></p></td></tr>\
                    <tr><td>Zeile</td><td>Zelle</td></tr></table>";
        let clean = sanitize_for_display(html, false);
        assert!(!clean.contains("<table") && !clean.contains("<td"));
        assert!(clean.contains("<p>Hallo</p><p>Welt <b>fett</b></p>"));
        assert!(clean.contains("Zeile<br>Zelle<br>"));
        assert!(!clean.contains("</p><br>"));
    }

    #[test]
    fn display_markdown_keeps_paragraphs_breaks_and_links() {
        let html = "<table><tr><td><p>Hallo,</p><p>Text mit <a href=\"https://x.de\">Link</a> und <b>fett</b>.</p>\
                    <p>Danke!<br>Anna<br>Support</p></td></tr></table>";
        let md = to_display_markdown(html, false);
        assert!(md.contains("Hallo,\n\nText mit [Link](https://x.de) und **fett**."), "{md}");
        assert!(md.contains("Danke!\\\nAnna\\\nSupport"), "{md}");
        let trailing = to_display_markdown("<p>weiter.<br></p><p>Danke</p>", false);
        assert_eq!(trailing, "weiter.\n\nDanke");
        let indented = to_display_markdown("<div>This is a test<br>\n  <br></div>", false);
        assert_eq!(indented, "This is a test");
        // Outlook and Spark: a div per line, an empty div is an empty line
        let lines = "<div>Grüße<br>Marc</div><div>Roothirsch GmbH</div><div>Brockhäger Str. 188</div>\
                     <div><br></div><div>Neuer Absatz</div>";
        assert_eq!(
            to_display_markdown(lines, false),
            "Grüße\\\nMarc\\\nRoothirsch GmbH\\\nBrockhäger Str. 188\n\nNeuer Absatz"
        );
    }
}
