//! Resizing pictures well: a Lanczos filter as wide as the scale needs, on
//! premultiplied pixels.
//!
//! Drawing a big picture small through a fixed-width filter (bicubic,
//! bilinear) skips most of its pixels, so edges come out jagged and thin
//! lines break up. A filter widened by the scale takes every source pixel
//! into account. Filtering premultiplied pixels keeps the color of
//! invisible pixels out of the edges (no dark fringes), and the result is
//! clamped back to valid premultiplied colors, since Lanczos overshoots a
//! little at sharp edges.

use image::imageops::{self, FilterType};
use image::{ImageBuffer, Rgba};
use tiny_skia::{IntSize, Pixmap};

/// Returns `source` resized to `width` × `height`, or none if either is 0.
pub(crate) fn resample(source: &Pixmap, width: u32, height: u32) -> Option<Pixmap> {
    if width == 0 || height == 0 {
        return None;
    }
    if (source.width(), source.height()) == (width, height) {
        return Some(source.clone());
    }
    // tiny-skia's pixels are premultiplied RGBA, which the filter treats
    // as four plain channels: exactly what premultiplied filtering wants.
    let premultiplied: ImageBuffer<Rgba<u8>, &[u8]> =
        ImageBuffer::from_raw(source.width(), source.height(), source.data())?;
    let resized = imageops::resize(&premultiplied, width, height, FilterType::Lanczos3);
    let mut data = resized.into_raw();
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = pixel[3];
        for channel in &mut pixel[..3] {
            *channel = (*channel).min(alpha);
        }
    }
    Pixmap::from_vec(data, IntSize::from_wh(width, height)?)
}

#[cfg(test)]
mod tests {
    use tiny_skia::PremultipliedColorU8;

    use super::*;

    /// A `side` × `side` picture whose pixels `paint` colors.
    fn picture(side: u32, paint: impl Fn(u32, u32) -> PremultipliedColorU8) -> Pixmap {
        let mut pixmap = Pixmap::new(side, side).expect("a size");
        for y in 0..side {
            for x in 0..side {
                let index = usize::try_from(y * side + x).expect("small");
                pixmap.pixels_mut()[index] = paint(x, y);
            }
        }
        pixmap
    }

    /// Opaque white or black.
    fn white_or_black(is_white: bool) -> PremultipliedColorU8 {
        let value = if is_white { 255 } else { 0 };
        PremultipliedColorU8::from_rgba(value, value, value, 255).expect("opaque")
    }

    #[test]
    fn a_fine_pattern_shrunk_averages_rather_than_aliasing() {
        // One-pixel stripes, shrunk 16 times: every pixel of the result
        // takes in all of its stripes, so it is grey, not black or white.
        let stripes = picture(256, |x, _| white_or_black(x % 2 == 0));
        let small = resample(&stripes, 16, 16).expect("resized");
        for pixel in small.pixels() {
            assert!(
                (96..=160).contains(&pixel.red()),
                "grey, not aliased: {}",
                pixel.red()
            );
        }
    }

    #[test]
    fn transparent_pixels_bring_no_dark_fringe() {
        // White on transparent black: an edge pixel half covered is half
        // transparent white, not grey.
        let square = picture(64, |x, _| {
            if x < 32 {
                white_or_black(true)
            } else {
                PremultipliedColorU8::TRANSPARENT
            }
        });
        let small = resample(&square, 8, 8).expect("resized");
        for pixel in small.pixels() {
            let color = pixel.demultiply();
            if color.alpha() > 16 {
                assert!(
                    color.red() > 230,
                    "white at alpha {}: {}",
                    color.alpha(),
                    color.red()
                );
            }
        }
    }

    #[test]
    fn the_result_is_valid_premultiplied_color() {
        let sharp = picture(100, |x, y| {
            if (x / 10 + y / 10) % 2 == 0 {
                white_or_black(true)
            } else {
                PremultipliedColorU8::TRANSPARENT
            }
        });
        let small = resample(&sharp, 13, 13).expect("resized");
        for pixel in small.pixels() {
            assert!(pixel.red() <= pixel.alpha() && pixel.blue() <= pixel.alpha());
        }
    }

    #[test]
    fn the_same_size_is_the_same_picture_and_zero_is_none() {
        let one = picture(4, |_, _| white_or_black(true));
        assert_eq!(resample(&one, 4, 4).expect("kept").data(), one.data());
        assert!(resample(&one, 0, 4).is_none());
    }
}
