//! Markdown (GitHub flavour: tables, strikethrough, autolinks) to mail HTML, for text an agent
//! writes: bold, lists and tables come out the way Outlook, Apple Mail and Just Mail's editor
//! show them.
//!
//! Blocks are `<div>` lines with an empty line (`<div><br></div>`) between them, as Outlook and
//! Spark write mail (`<p>` collapses in Outlook); a single line break stays a line break, as in a
//! mail. Everything is styled inline, since mail clients drop `<style>`. Pictures stay out
//! (remote images are tracking pixels to most mail clients), their `alt` text stands in.

use html_escape::{encode_double_quoted_attribute as attr, encode_text as text};
use markdown::mdast::{AlignKind, Node};

/// A blank line between two blocks.
const BLANK: &str = "<div><br></div>";
/// The inline styles of a table's cells, its header cells and a quote: also what Just Mail's
/// editor gives the ones made there.
pub const CELL: &str = "border:1px solid #d0d0d0;padding:4px 10px;vertical-align:top";
pub const HEAD_CELL: &str = "border:1px solid #d0d0d0;padding:4px 10px;vertical-align:top;font-weight:bold;background-color:#f3f3f3";
pub const QUOTE: &str = "margin:0 0 0 4px;padding-left:10px;border-left:3px solid #d0d0d0";
const MONO: &str = "font-family:Menlo,Consolas,monospace";

/// Mail HTML for the author's part of a draft (see [`crate::html::compose_blocks`]).
pub fn to_mail_html(md: &str) -> String {
    // Without MDX, GFM Markdown always parses: anything unknown stays text.
    let root = markdown::to_mdast(md, &markdown::ParseOptions::gfm())
        .unwrap_or_else(|_| Node::Text(markdown::mdast::Text { value: md.to_string(), position: None }));
    blocks(&root)
}

/// The children of `node` as blocks, a blank line between each two.
fn blocks(node: &Node) -> String {
    let parts: Vec<String> = children(node).iter().map(block).filter(|b| !b.is_empty()).collect();
    parts.join(&format!("\n{BLANK}\n"))
}

fn children(node: &Node) -> &[Node] {
    node.children().map_or(&[], Vec::as_slice)
}

fn block(node: &Node) -> String {
    match node {
        Node::Paragraph(_) => format!("<div>{}</div>", inline(node)),
        Node::Heading(h) => {
            let size = match h.depth {
                1 => "15pt",
                2 => "13pt",
                _ => "11pt",
            };
            format!("<div style=\"font-size:{size};font-weight:bold\">{}</div>", inline(node))
        }
        Node::List(list) => {
            let tag = if list.ordered { "ol" } else { "ul" };
            let start = list.start.filter(|s| list.ordered && *s != 1).map_or(String::new(), |s| format!(" start=\"{s}\""));
            let items: String = children(node).iter().map(|item| format!("<li>{}</li>", list_item(item))).collect();
            format!("<{tag}{start} style=\"margin-top:0;margin-bottom:0\">{items}</{tag}>")
        }
        Node::Table(table) => table_html(node, &table.align),
        Node::Blockquote(_) => format!("<blockquote style=\"{QUOTE}\">{}</blockquote>", blocks(node)),
        Node::Code(code) => {
            format!("<pre style=\"{MONO};font-size:10pt;margin:0;white-space:pre-wrap\">{}</pre>", text(&code.value))
        }
        Node::ThematicBreak(_) => "<hr>".into(),
        Node::Html(html) => html.value.clone(),
        Node::Definition(_) => String::new(),
        _ if node.children().is_some() => blocks(node),
        _ => format!("<div>{}</div>", inline(node)),
    }
}

/// A list item: its paragraphs as lines, nested lists below them.
fn list_item(item: &Node) -> String {
    let mut out = String::new();
    for (i, child) in children(item).iter().enumerate() {
        match child {
            Node::Paragraph(_) => {
                if i > 0 {
                    out.push_str("<br>");
                }
                out.push_str(&inline(child));
            }
            _ => out.push_str(&block(child)),
        }
    }
    out
}

fn table_html(node: &Node, align: &[AlignKind]) -> String {
    let mut rows = String::new();
    for (r, row) in children(node).iter().enumerate() {
        rows.push_str("<tr>");
        for (c, cell) in children(row).iter().enumerate() {
            let (tag, style) = if r == 0 { ("th", HEAD_CELL) } else { ("td", CELL) };
            let side = match align.get(c) {
                Some(AlignKind::Right) => "right",
                Some(AlignKind::Center) => "center",
                _ => "left",
            };
            rows.push_str(&format!("<{tag} style=\"{style};text-align:{side}\">{}</{tag}>", inline(cell)));
        }
        rows.push_str("</tr>");
    }
    format!("<table cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse\">{rows}</table>")
}

/// The inline content of `node`.
fn inline(node: &Node) -> String {
    children(node).iter().map(span).collect()
}

fn span(node: &Node) -> String {
    match node {
        // a line break in a paragraph is one in the mail too
        Node::Text(t) => text(&t.value).replace('\n', "<br>\n"),
        Node::Strong(_) => format!("<b>{}</b>", inline(node)),
        Node::Emphasis(_) => format!("<i>{}</i>", inline(node)),
        Node::Delete(_) => format!("<s>{}</s>", inline(node)),
        Node::InlineCode(code) => format!("<code style=\"{MONO}\">{}</code>", text(&code.value)),
        Node::Break(_) => "<br>\n".into(),
        Node::Link(link) if safe_link(&link.url) => format!("<a href=\"{}\">{}</a>", attr(&link.url), inline(node)),
        Node::Image(image) => text(&image.alt).into_owned(),
        Node::Html(html) => html.value.clone(),
        _ => inline(node),
    }
}

/// Links a mail may carry (no `javascript:` and the like).
fn safe_link(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["https:", "http:", "mailto:", "tel:"].iter().any(|scheme| lower.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::to_mail_html;

    #[test]
    fn paragraphs_are_lines_with_a_blank_line_between() {
        let html = to_mail_html("Hallo Hansueli,\n\nkein Ding.\nZweite Zeile.");
        assert_eq!(html, "<div>Hallo Hansueli,</div>\n<div><br></div>\n<div>kein Ding.<br>\nZweite Zeile.</div>");
    }

    #[test]
    fn tables_get_borders_and_a_header_row() {
        let html = to_mail_html("| Uhrzeit | Schritt |\n|---|--:|\n| 15:54 | **Bestellung** |");
        assert!(html.starts_with("<table cellpadding=\"0\" cellspacing=\"0\" style=\"border-collapse:collapse\"><tr><th style="));
        assert!(html.contains(">Uhrzeit</th>"));
        assert!(html.contains("text-align:right\"><b>Bestellung</b></td></tr></table>"));
    }

    #[test]
    fn lists_bold_links_and_escaping() {
        let html = to_mail_html("- **a** & <b\n- [Shop](https://example.com)\n- [x](javascript:alert(1))");
        assert_eq!(
            html,
            "<ul style=\"margin-top:0;margin-bottom:0\"><li><b>a</b> &amp; &lt;b</li>\
             <li><a href=\"https://example.com\">Shop</a></li><li>x</li></ul>"
        );
        assert_eq!(to_mail_html("1. a\n2. b"), "<ol style=\"margin-top:0;margin-bottom:0\"><li>a</li><li>b</li></ol>");
    }
}
