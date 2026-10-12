//! Pictures pasted into a draft: made into what every mail program shows (PNG, JPEG or GIF),
//! small enough for one Graph request, with a little preview for the composer.

use std::io::Cursor;
use std::sync::Arc;

use gpui_kit::ImageFormat;
use image::DynamicImage;
use image::imageops::FilterType;
use jm_core::graph::MAX_ATTACHMENT_BYTES;

/// Height of a preview in pixels (twice what the composer shows, for Retina screens).
const PREVIEW_H: u32 = 80;

pub struct Prepared {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// Of the pasted picture; a shrunken one still shows this big.
    pub size: (u32, u32),
    pub preview: Option<Arc<gpui_kit::Image>>,
}

/// File extension of a pasted picture in `format`, as [`prepare`] leaves it.
pub fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        _ => "png",
    }
}

/// JPEG and GIF stay what they are, anything else becomes PNG. Too big for one request, it is
/// halved until it fits (a Retina screenshot then has the size it shows at).
pub fn prepare(format: ImageFormat, bytes: Vec<u8>) -> Result<Prepared, String> {
    let (kind, content_type, keep) = match format {
        ImageFormat::Jpeg => (image::ImageFormat::Jpeg, "image/jpeg", true),
        ImageFormat::Gif => (image::ImageFormat::Gif, "image/gif", true),
        ImageFormat::Png => (image::ImageFormat::Png, "image/png", true),
        _ => (image::ImageFormat::Png, "image/png", false),
    };
    let source = match format {
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        _ if keep => kind,
        _ => return Err("this kind of picture can't go into a mail".into()),
    };
    let mut picture = image::load_from_memory_with_format(&bytes, source).map_err(|e| e.to_string())?;
    let size = (picture.width(), picture.height());
    let preview = preview(&picture);
    let mut bytes = if keep { bytes } else { encode(&picture, kind)? };
    let mut halved = 0;
    while bytes.len() > MAX_ATTACHMENT_BYTES {
        if kind == image::ImageFormat::Gif || halved == 4 {
            return Err("the picture is larger than 3 MB".into());
        }
        picture = picture.resize(picture.width() / 2, picture.height() / 2, FilterType::Triangle);
        bytes = encode(&picture, kind)?;
        halved += 1;
    }
    Ok(Prepared { bytes, content_type, size, preview })
}

/// A preview of a picture already in a draft.
pub fn preview_of(bytes: &[u8]) -> Option<Arc<gpui_kit::Image>> {
    preview(&image::load_from_memory(bytes).ok()?)
}

fn preview(picture: &DynamicImage) -> Option<Arc<gpui_kit::Image>> {
    let small = picture.thumbnail(PREVIEW_H * 4, PREVIEW_H);
    let bytes = encode(&small, image::ImageFormat::Png).ok()?;
    Some(Arc::new(gpui_kit::Image::from_bytes(ImageFormat::Png, bytes)))
}

fn encode(picture: &DynamicImage, format: image::ImageFormat) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    let written = if format == image::ImageFormat::Jpeg {
        // JPEG has no alpha channel
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&picture.to_rgb8())
    } else {
        picture.write_to(&mut out, format)
    };
    written.map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        // noise, so PNG can't squeeze it
        let mut seed = 7u32;
        let picture = image::RgbImage::from_fn(width, height, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
        });
        encode(&DynamicImage::ImageRgb8(picture), format).unwrap()
    }

    #[test]
    fn small_png_stays_as_it_is() {
        let bytes = picture(40, 20, image::ImageFormat::Png);
        let prepared = prepare(ImageFormat::Png, bytes.clone()).unwrap();
        assert_eq!(prepared.bytes, bytes);
        assert_eq!((prepared.content_type, prepared.size), ("image/png", (40, 20)));
        assert!(prepared.preview.is_some());
    }

    #[test]
    fn tiff_becomes_png() {
        let prepared = prepare(ImageFormat::Tiff, picture(30, 10, image::ImageFormat::Tiff)).unwrap();
        assert_eq!(image::guess_format(&prepared.bytes).unwrap(), image::ImageFormat::Png);
        assert_eq!(extension(ImageFormat::Tiff), "png");
    }

    #[test]
    fn too_big_is_halved_until_it_fits() {
        // about 4.7 MB as PNG
        let prepared = prepare(ImageFormat::Png, picture(1400, 1120, image::ImageFormat::Png)).unwrap();
        assert!(prepared.bytes.len() <= MAX_ATTACHMENT_BYTES);
        assert_eq!(prepared.size, (1400, 1120));
        assert_eq!(image::load_from_memory(&prepared.bytes).unwrap().width(), 700);
    }
}
