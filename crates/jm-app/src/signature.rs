//! A faithful preview of an HTML signature: its text as Markdown (a `<div>` is a line, as in
//! Spark) and its embedded `data:` images, which the text view cannot show, as real images.

use std::sync::Arc;

use gpui_kit::base::v_flex;
use gpui_kit::component::text::TextView;
use gpui_kit::*;

enum Part {
    Text(SharedString),
    Image(Arc<Image>, f32, f32),
}

pub struct Preview {
    id: SharedString,
    parts: Vec<Part>,
}

/// Images wider than this are scaled down (Spark shows the 146 px logo about this size).
const MAX_W: f32 = 140.;

impl Preview {
    pub fn new(id: impl Into<SharedString>, html: &str) -> Self {
        // Convert the whole signature first (cutting the HTML at an image would break its
        // `<div>` lines), with each image replaced by a placeholder line, then split there.
        let (html, images) = jm_core::html::inline_data_images(html, "preview", "preview");
        let mut marked = String::with_capacity(html.len());
        let mut rest = html.as_str();
        while let Some(start) = rest.to_ascii_lowercase().find("<img") {
            let end = rest[start..].find('>').map(|e| start + e + 1).unwrap_or(rest.len());
            marked.push_str(&rest[..start]);
            let tag = &rest[start..end];
            if let Some(ix) = images.iter().position(|i| tag.contains(&i.content_id)) {
                marked.push_str(&format!("<br>{}{ix}<br>", PLACEHOLDER));
            }
            rest = &rest[end..];
        }
        marked.push_str(rest);
        let md = jm_core::html::to_display_markdown(&marked, false);

        let mut parts = Vec::new();
        let mut text = md.as_str();
        while let Some(at) = text.find(PLACEHOLDER) {
            push_text(&mut parts, &text[..at]);
            let digits: String = text[at + PLACEHOLDER.len()..].chars().take_while(char::is_ascii_digit).collect();
            if let Some(image) = digits.parse::<usize>().ok().and_then(|ix| images.get(ix)) {
                let (w, h) = png_size(&image.bytes).unwrap_or((MAX_W as u32, MAX_W as u32));
                let scale = (MAX_W / w as f32).min(1.0);
                let format = if image.content_type.contains("jpeg") { ImageFormat::Jpeg } else { ImageFormat::Png };
                parts.push(Part::Image(
                    Arc::new(Image::from_bytes(format, image.bytes.clone())),
                    w as f32 * scale,
                    h as f32 * scale,
                ));
            }
            text = &text[at + PLACEHOLDER.len() + digits.len()..];
        }
        push_text(&mut parts, text);
        Self { id: id.into(), parts }
    }

    pub fn render(&self) -> impl IntoElement + use<> {
        let mut col = v_flex().gap_2();
        for (i, part) in self.parts.iter().enumerate() {
            col = match part {
                Part::Text(md) => {
                    col.child(TextView::markdown(ElementId::Name(format!("{}-{i}", self.id).into()), md.clone()))
                }
                Part::Image(image, w, h) => col.child(img(image.clone()).w(px(*w)).h(px(*h))),
            };
        }
        col
    }
}

const PLACEHOLDER: &str = "JMSIGNATUREIMAGE";

/// A Markdown piece between images, without the line breaks that led up to or away from them.
fn push_text(parts: &mut Vec<Part>, md: &str) {
    let md = md.trim_matches(|c: char| c == '\\' || c.is_whitespace());
    if !md.is_empty() {
        parts.push(Part::Text(md.to_string().into()));
    }
}

/// Width and height from a PNG header.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[1..4] != b"PNG" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}
