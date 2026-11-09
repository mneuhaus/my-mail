//! Pictures for the text of a draft, pasted or from disk: made into what every mail program
//! shows (PNG, JPEG or GIF), upright, and small enough for one Graph request.

use std::io::Cursor;
use std::path::Path;

use gpui_kit::ImageFormat;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder as _};
use jm_core::graph::MAX_ATTACHMENT_BYTES;

#[derive(Debug)]
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// Of its file name.
    pub extension: &'static str,
    /// Of the picture as it came; a shrunken one still shows this big.
    pub size: (u32, u32),
}

/// Whether the file is a picture for the text, by its name (anything else is attached).
pub fn is_picture(path: &Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif"].contains(&extension.as_str())
}

/// A pasted picture.
pub fn prepare(format: ImageFormat, bytes: Vec<u8>) -> Result<Prepared, String> {
    let source = match format {
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        _ => return Err(UNKNOWN.into()),
    };
    for_mail(source, bytes)
}

/// A picture from disk. What only macOS can read (HEIC from an iPhone) becomes a JPEG first.
pub fn from_file(path: &Path) -> Result<Prepared, String> {
    use image::ImageFormat::{Bmp, Gif, Jpeg, Png, Tiff, WebP};
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match image::guess_format(&bytes) {
        Ok(source) if [Png, Jpeg, Gif, WebP, Bmp, Tiff].contains(&source) => for_mail(source, bytes),
        _ => for_mail(Jpeg, as_jpeg(path)?),
    }
}

const UNKNOWN: &str = "this kind of picture can't go into a mail";

/// The picture as a JPEG, made by `sips` (part of macOS).
#[cfg(target_os = "macos")]
fn as_jpeg(path: &Path) -> Result<Vec<u8>, String> {
    let made = std::env::temp_dir().join(format!("just-mail-{}.jpg", jm_core::html::content_id_token()));
    let done = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "85"])
        .arg(path)
        .arg("--out")
        .arg(&made)
        .output();
    let bytes = std::fs::read(&made);
    let _ = std::fs::remove_file(&made);
    match (done, bytes) {
        (Ok(done), Ok(bytes)) if done.status.success() => Ok(bytes),
        _ => Err(UNKNOWN.into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn as_jpeg(_: &Path) -> Result<Vec<u8>, String> {
    Err(UNKNOWN.into())
}

/// JPEG, GIF and PNG stay what they are, anything else becomes PNG. A photo taken sideways is
/// turned upright (not every mail program reads how it was held). Too big for one request, the
/// picture is halved until it fits (a Retina screenshot then has the size it shows at).
fn for_mail(source: image::ImageFormat, bytes: Vec<u8>) -> Result<Prepared, String> {
    use image::ImageFormat::{Gif, Jpeg, Png};
    let (kind, content_type, extension) = match source {
        Jpeg => (Jpeg, "image/jpeg", "jpg"),
        Gif => (Gif, "image/gif", "gif"),
        _ => (Png, "image/png", "png"),
    };
    let mut decoder = image::ImageReader::with_format(Cursor::new(&bytes), source).into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    picture.apply_orientation(orientation);
    let size = (picture.width(), picture.height());
    let keep = kind == source && orientation == Orientation::NoTransforms;
    let mut bytes = if keep { bytes } else { encode(&picture, kind)? };
    let mut halved = 0;
    while bytes.len() > MAX_ATTACHMENT_BYTES {
        if kind == Gif || halved == 4 {
            return Err("the picture is larger than 3 MB".into());
        }
        picture = picture.resize(picture.width() / 2, picture.height() / 2, FilterType::Triangle);
        bytes = encode(&picture, kind)?;
        halved += 1;
    }
    Ok(Prepared { bytes, content_type, extension, size })
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
        assert_eq!((prepared.content_type, prepared.extension, prepared.size), ("image/png", "png", (40, 20)));
    }

    #[test]
    fn tiff_becomes_png() {
        let prepared = prepare(ImageFormat::Tiff, picture(30, 10, image::ImageFormat::Tiff)).unwrap();
        assert_eq!(image::guess_format(&prepared.bytes).unwrap(), image::ImageFormat::Png);
        assert_eq!(prepared.extension, "png");
    }

    #[test]
    fn too_big_is_halved_until_it_fits() {
        // about 4.7 MB as PNG
        let prepared = prepare(ImageFormat::Png, picture(1400, 1120, image::ImageFormat::Png)).unwrap();
        assert!(prepared.bytes.len() <= MAX_ATTACHMENT_BYTES);
        assert_eq!(prepared.size, (1400, 1120));
        assert_eq!(image::load_from_memory(&prepared.bytes).unwrap().width(), 700);
    }

    #[test]
    fn a_photo_taken_sideways_is_turned_upright() {
        // a JPEG, 40 wide and 20 high, whose EXIF says: turn me by 90° to show me
        let jpeg = picture(40, 20, image::ImageFormat::Jpeg);
        let exif: &[u8] = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0";
        let mut turned = jpeg[..2].to_vec();
        turned.extend([0xFF, 0xE1]);
        turned.extend(((exif.len() + 2) as u16).to_be_bytes());
        turned.extend(exif);
        turned.extend(&jpeg[2..]);
        let prepared = prepare(ImageFormat::Jpeg, turned).unwrap();
        assert_eq!(prepared.size, (20, 40));
        let upright = image::load_from_memory(&prepared.bytes).unwrap();
        assert_eq!((upright.width(), upright.height()), (20, 40));
    }

    /// HEIC, as an iPhone takes photos, is read by macOS only.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_heic_file_becomes_a_jpeg() {
        let dir = std::env::temp_dir().join(format!("just-mail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png, heic) = (dir.join("photo.png"), dir.join("photo.heic"));
        std::fs::write(&png, picture(64, 32, image::ImageFormat::Png)).unwrap();
        let made = std::process::Command::new("/usr/bin/sips").args(["-s", "format", "heic"]).arg(&png).arg("--out").arg(&heic).output();
        assert!(made.is_ok_and(|m| m.status.success()), "sips could not make the HEIC to test with");
        let prepared = from_file(&heic).unwrap();
        assert_eq!((prepared.content_type, prepared.extension, prepared.size), ("image/jpeg", "jpg", (64, 32)));
        assert!(from_file(&dir.join("missing.heic")).is_err());
        std::fs::write(dir.join("text.heic"), b"no picture").unwrap();
        assert_eq!(from_file(&dir.join("text.heic")).unwrap_err(), UNKNOWN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_are_pictures_by_their_names() {
        assert!(is_picture(Path::new("/tmp/IMG_0001.HEIC")) && is_picture(Path::new("shot.png")));
        assert!(!is_picture(Path::new("offer.pdf")) && !is_picture(Path::new("png")));
    }
}
