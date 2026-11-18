//! Images a style draws with: an `XPM` (X `PixMap`) file's pixels, which is what
//! Fluxbox styles use for textures and button pictures (02 §10.1).

use std::fmt;

use crate::color::Color;

/// A picture: its pixels, straight (not premultiplied) alpha, row by row.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Image {
    /// How many pixels wide.
    width: u32,
    /// How many pixels tall.
    height: u32,
    /// Every pixel, left to right and top to bottom.
    pixels: Vec<Color>,
}

impl fmt::Debug for Image {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The pixels would drown whatever holds the image.
        write!(formatter, "Image({}x{})", self.width, self.height)
    }
}

/// An image that cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an XPM image: {reason}")]
pub struct ImageError {
    /// What is wrong.
    reason: &'static str,
}

impl ImageError {
    /// Returns the error for `reason`.
    const fn new(reason: &'static str) -> Self {
        Self { reason }
    }
}

impl Image {
    /// The widest or tallest image read: past this, a file is a mistake.
    const MOST_PIXELS_ACROSS: usize = 4096;

    /// Returns how many pixels wide it is.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns how many pixels tall it is.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns every pixel, left to right and top to bottom.
    #[must_use]
    pub fn pixels(&self) -> &[Color] {
        &self.pixels
    }

    /// Returns the average color of the pixels that are not clear, as an
    /// opaque color; none if every pixel is clear.
    #[must_use]
    pub fn average_color(&self) -> Option<Color> {
        let mut total = [0_u64; 3];
        let mut count = 0_u64;
        for pixel in self.pixels.iter().filter(|pixel| pixel.alpha > 0) {
            total[0] += u64::from(pixel.red);
            total[1] += u64::from(pixel.green);
            total[2] += u64::from(pixel.blue);
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let [red, green, blue] = total.map(|sum| u8::try_from(sum / count).unwrap_or(u8::MAX));
        Some(Color::rgb(red, green, blue))
    }

    /// Reads an XPM file's `text` (XPM version 3, one or two characters a
    /// pixel). A color that is `None` is clear.
    ///
    /// # Errors
    ///
    /// Returns an error if the text is not a well-formed XPM, or has a
    /// color vaht cannot read.
    pub fn from_xpm(text: &str) -> Result<Self, ImageError> {
        let mut strings = text
            .split('"')
            .enumerate()
            .filter(|(index, _)| index % 2 == 1)
            .map(|(_, part)| part);
        let header: Vec<usize> = strings
            .next()
            .ok_or(ImageError::new("no header"))?
            .split_whitespace()
            .map_while(|number| number.parse().ok())
            .collect();
        let [width, height, colors, per_pixel, ..] = header[..] else {
            return Err(ImageError::new(
                "the header is not `width height colors chars`",
            ));
        };
        if width == 0
            || height == 0
            || width > Self::MOST_PIXELS_ACROSS
            || height > Self::MOST_PIXELS_ACROSS
            || !(1..=2).contains(&per_pixel)
        {
            return Err(ImageError::new(
                "the size or characters per pixel are not usable",
            ));
        }
        let mut table: Vec<(Vec<char>, Color)> = Vec::with_capacity(colors);
        for _ in 0..colors {
            let entry = strings
                .next()
                .ok_or(ImageError::new("a color is missing"))?;
            let key: Vec<char> = entry.chars().take(per_pixel).collect();
            let words: String = entry.chars().skip(per_pixel).collect();
            table.push((key, Self::color_of(&words)?));
        }
        let mut pixels = Vec::with_capacity(width * height);
        for _ in 0..height {
            let row: Vec<char> = strings
                .next()
                .ok_or(ImageError::new("a row of pixels is missing"))?
                .chars()
                .collect();
            if row.len() < width * per_pixel {
                return Err(ImageError::new("a row of pixels is too short"));
            }
            for pixel in row.chunks(per_pixel).take(width) {
                let color = table
                    .iter()
                    .find(|(key, _)| key == pixel)
                    .map(|(_, color)| *color)
                    .ok_or(ImageError::new("a pixel has no color"))?;
                pixels.push(color);
            }
        }
        Ok(Self {
            width: u32::try_from(width).map_err(|_unused| ImageError::new("too wide"))?,
            height: u32::try_from(height).map_err(|_unused| ImageError::new("too tall"))?,
            pixels,
        })
    }

    /// Returns the color a color entry's words (`c #7C7C7C`, `m white c
    /// None`) give: the `c` (color) visual's; clear for `None`.
    fn color_of(words: &str) -> Result<Color, ImageError> {
        let mut words = words.split_whitespace();
        while let Some(word) = words.next() {
            if word != "c" {
                continue;
            }
            let value = words
                .next()
                .ok_or(ImageError::new("a color has no value"))?;
            if value.eq_ignore_ascii_case("none") {
                return Ok(Color::TRANSPARENT);
            }
            let value = match value.strip_prefix('#') {
                // `#rrrrggggbbbb` is sixteen bits a channel: keep the high eight.
                Some(digits) if digits.len() == 12 && digits.is_ascii() => {
                    format!("#{}{}{}", &digits[0..2], &digits[4..6], &digits[8..10])
                }
                _ => value.to_owned(),
            };
            return value
                .parse()
                .map_err(|_unused| ImageError::new("a color is not one vaht reads"));
        }
        Err(ImageError::new("a color has no `c` value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two by two image: black and white above, clear below.
    const SMALL: &str = "/* XPM */\nstatic char * a[] = {\n\"2 2 3 1\",\n\" \tc None\",\n\".\tc #000000\",\n\"+\tc #ffffff\",\n\".+\",\n\"  \"};";

    #[test]
    fn an_xpm_is_read_pixel_by_pixel_with_none_clear() {
        let image = Image::from_xpm(SMALL).expect("valid");
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(
            image.pixels(),
            [
                Color::BLACK,
                Color::WHITE,
                Color::TRANSPARENT,
                Color::TRANSPARENT
            ]
        );
    }

    #[test]
    fn the_average_ignores_clear_pixels() {
        let image = Image::from_xpm(SMALL).expect("valid");
        assert_eq!(image.average_color(), Some(Color::rgb(127, 127, 127)));
    }

    #[test]
    fn sixteen_bit_and_named_colors_are_read() {
        let text = "\"2 1 2 1\",\n\"a\tc #ffff00000000\",\n\"b\tm black c SteelBlue\",\n\"ab\"";
        let image = Image::from_xpm(text).expect("valid");
        assert_eq!(image.pixels()[0], Color::rgb(255, 0, 0));
        assert_eq!(image.pixels()[1], "SteelBlue".parse().expect("a color"));
    }

    #[test]
    fn two_characters_a_pixel_work() {
        let text = "\"2 1 2 2\",\n\"aa\tc #ff0000\",\n\"bb\tc #0000ff\",\n\"aabb\"";
        let image = Image::from_xpm(text).expect("valid");
        assert_eq!(
            image.pixels(),
            [Color::rgb(255, 0, 0), Color::rgb(0, 0, 255)]
        );
    }

    #[test]
    fn malformed_images_are_refused() {
        for bad in [
            "not an xpm",
            "\"0 0 0 1\"",
            "\"1 1 1 1\",\n\"a\tc #000000\"",
            "\"1 1 1 1\",\n\"a\tc #000000\",\n\"b\"",
            "\"1 1 1 1\",\n\"a\tc notacolor\",\n\"a\"",
            "\"9999 1 1 1\",\n\"a\tc #000000\",\n\"a\"",
        ] {
            assert!(Image::from_xpm(bad).is_err(), "{bad}");
        }
    }
}
