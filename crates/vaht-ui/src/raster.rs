//! Raster images decoded into pixmaps, whatever their format.
//!
//! The format is read from the data itself, so a JPEG named `.png` still
//! loads; only a format with no signature of its own (TGA) is known by the
//! file's extension. Every common format is understood: PNG,
//! JPEG, GIF (its first frame), WebP, BMP, TIFF, TGA, PNM, QOI and ICO.

use std::path::Path;

use tiny_skia::{IntSize, Pixmap};

/// Decodes `data`, an image in any format vaht reads, read from `path`,
/// into premultiplied pixels.
///
/// # Errors
///
/// Returns the decoder's reason if the format is unknown or the data is
/// damaged.
///
/// # Panics
///
/// Never: a channel times an alpha, over 255, fits in a byte.
pub fn decode(data: &[u8], path: &Path) -> Result<Pixmap, image::ImageError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    if reader.format().is_none()
        && let Ok(format) = image::ImageFormat::from_path(path)
    {
        reader.set_format(format);
    }
    let decoded = reader.decode()?.into_rgba8();
    let (width, height) = decoded.dimensions();
    let size = IntSize::from_wh(width, height).ok_or_else(|| {
        image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ))
    })?;
    let mut pixels = decoded.into_raw();
    // tiny-skia keeps premultiplied alpha.
    let (rgba, _) = pixels.as_chunks_mut::<4>();
    for [red, green, blue, alpha] in rgba {
        let premultiply = |channel: &mut u8| {
            let product = u16::from(*channel) * u16::from(*alpha);
            *channel = u8::try_from((product + 127) / 255).expect("a product over 255 fits");
        };
        premultiply(red);
        premultiply(green);
        premultiply(blue);
    }
    Pixmap::from_vec(pixels, size).ok_or_else(|| {
        image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a 2 by 1 image, red then half-transparent white, as `format`.
    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut image = image::RgbaImage::new(2, 1);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, image::Rgba([255, 255, 255, 128]));
        let mut data = std::io::Cursor::new(Vec::new());
        let image = image::DynamicImage::ImageRgba8(image);
        // JPEG has no alpha to write.
        let image = if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        image
            .write_to(&mut data, format)
            .expect("the test image encodes");
        data.into_inner()
    }

    #[test]
    fn a_png_decodes_premultiplied() {
        let pixmap =
            decode(&encoded(image::ImageFormat::Png), Path::new("x.jpg")).expect("decodes");
        let red = pixmap.pixel(0, 0).expect("inside");
        assert_eq!((red.red(), red.alpha()), (255, 255));
        let white = pixmap.pixel(1, 0).expect("inside");
        assert_eq!((white.red(), white.alpha()), (128, 128));
    }

    #[test]
    fn other_formats_are_found_by_their_contents_or_name() {
        for (format, name) in [
            (image::ImageFormat::Bmp, "a"),
            (image::ImageFormat::Qoi, "b"),
            (image::ImageFormat::Tga, "c.tga"),
            (image::ImageFormat::Jpeg, "d"),
            (image::ImageFormat::Gif, "e"),
            (image::ImageFormat::WebP, "f"),
        ] {
            let pixmap = decode(&encoded(format), Path::new(name)).expect("decodes");
            assert_eq!(pixmap.width(), 2, "{format:?}");
        }
        assert!(decode(b"not an image", Path::new("d.png")).is_err());
    }
}
