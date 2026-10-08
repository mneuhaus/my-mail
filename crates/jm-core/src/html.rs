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

/// The marked author block followed by the signature block (if any).
pub fn compose_blocks(body_html: &str, signature_text: &str) -> String {
    let mut out = format!("<div id=\"{BODY_ID}\" style=\"{FONT}\">{body_html}</div>\n");
    if !signature_text.trim().is_empty() {
        out.push_str(&format!(
            "<div id=\"{SIGNATURE_ID}\" style=\"{FONT}\"><br>\n{}</div>\n",
            text_to_html(signature_text.trim_end())
        ));
    }
    out
}

/// A complete HTML document for a new mail.
pub fn new_document(body_html: &str, signature_text: &str) -> String {
    format!("<html><body>\n{}</body></html>", compose_blocks(body_html, signature_text))
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

/// Replace the author's part of a draft. Without markers the whole body is replaced
/// (keeping nothing of the old content).
pub fn replace_author_html(document: &str, body_html: &str, signature_text: &str) -> String {
    match element_inner(document, BODY_ID) {
        Some((s, e)) => format!("{}{}{}", &document[..s], body_html, &document[e..]),
        None => new_document(body_html, signature_text),
    }
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
    let is_space = |l: &str| l.is_empty() || l == "\\";
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
/// line break. Expects normalized HTML (ammonia output: lowercase tags, quoted attributes).
fn flatten_layout_tables(html: &str) -> String {
    const DROP: [&str; 9] = ["table", "tbody", "thead", "tfoot", "tr", "td", "th", "center", "caption"];
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
                if closing && (name == "td" || name == "th") {
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
        let changed = replace_author_html(&doc, "neu", "Marc");
        assert_eq!(author_html(&changed), Some("neu"));
        assert!(changed.contains("id=\"jm-signature\""));
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
    }
}
