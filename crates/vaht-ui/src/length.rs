//! Lengths in a style: a number of pixels, or a multiple of the font size.
//!
//! Every length a style file writes (padding, spacing, a bar's height, a
//! border's width, a corner's radius) is either a plain number of logical
//! pixels, or a string of a number and `em`: `"1.5em"` is one and a half
//! times the size of the element's font, so the room follows the text.
//! `"12px"` is the plain number written out.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};

/// A length in a style file, until the font it may be relative to is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Length {
    /// A number of logical pixels.
    Pixels(u32),
    /// A multiple of the element's font size, in thousandths.
    Em(u32),
}

impl Length {
    /// Returns the length in pixels, where an em is `em` pixels (the size
    /// of the element's font), rounded to the nearest.
    #[must_use]
    pub fn to_pixels(self, em: f32) -> u32 {
        match self {
            Self::Pixels(pixels) => pixels,
            Self::Em(thousandths) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss,
                    reason = "a style's lengths are small and positive"
                )]
                let pixels = (thousandths as f32 / 1000.0 * em).round() as u32;
                pixels
            }
        }
    }

    /// Reads a length written as text: `12`, `12px` or `1.5em`.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(number) = text.strip_suffix("em") {
            let em: f32 = number.trim().parse().ok().filter(|em: &f32| *em >= 0.0)?;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a length in em is small and positive"
            )]
            let thousandths = (em * 1000.0).round() as u32;
            return Some(Self::Em(thousandths));
        }
        text.strip_suffix("px")
            .unwrap_or(text)
            .trim()
            .parse()
            .ok()
            .map(Self::Pixels)
    }
}

impl<'de> serde::Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a number, or text such as `1.5em`.
        struct LengthVisitor;

        impl Visitor<'_> for LengthVisitor {
            type Value = Length;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a length: a number of pixels, or text with a unit, `1.5em` (times the \
                     font's size) or `12px`",
                )
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<Length, E> {
                u32::try_from(number)
                    .map(Length::Pixels)
                    .map_err(|_out_of_range| {
                        E::invalid_value(de::Unexpected::Signed(number), &self)
                    })
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<Length, E> {
                u32::try_from(number)
                    .map(Length::Pixels)
                    .map_err(|_out_of_range| {
                        E::invalid_value(de::Unexpected::Unsigned(number), &self)
                    })
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Length, E> {
                Length::parse(text)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }

        deserializer.deserialize_any(LengthVisitor)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Reads `text` as the one value of a TOML document.
    fn read(text: &str) -> Result<Length, toml::de::Error> {
        #[derive(serde::Deserialize)]
        struct One {
            length: Length,
        }
        toml::from_str::<One>(&format!("length = {text}")).map(|one| one.length)
    }

    #[rstest]
    #[case("4", Length::Pixels(4))]
    #[case("\"4\"", Length::Pixels(4))]
    #[case("\"12px\"", Length::Pixels(12))]
    #[case("\"1.5em\"", Length::Em(1500))]
    #[case("\"0.25 em\"", Length::Em(250))]
    #[case("0", Length::Pixels(0))]
    fn lengths_are_read_in_pixels_or_em(#[case] text: &str, #[case] expected: Length) {
        assert_eq!(read(text).expect("valid"), expected);
    }

    #[rstest]
    #[case("-3")]
    #[case("\"1.5rem\"")]
    #[case("\"em\"")]
    #[case("\"-1em\"")]
    #[case("1.5")]
    #[case("true")]
    fn anything_else_is_refused_and_says_what_a_length_is(#[case] text: &str) {
        let error = read(text).expect_err(text).to_string();
        assert!(error.contains("a length"), "{error}");
    }

    #[test]
    fn an_em_is_the_fonts_size_rounded_to_a_pixel() {
        assert_eq!(Length::Em(1000).to_pixels(13.0), 13);
        assert_eq!(Length::Em(500).to_pixels(13.0), 7, "6.5 rounds up");
        assert_eq!(
            Length::Pixels(5).to_pixels(13.0),
            5,
            "pixels ignore the font"
        );
    }
}
