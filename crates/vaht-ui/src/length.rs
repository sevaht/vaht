//! Lengths in a style: pixels, the font's size, the font's height, or a sum.
//!
//! Every length a style file writes (padding, spacing, a bar's height, a
//! border's width, a corner's radius) is a plain number of logical pixels, or
//! text of terms added and subtracted, each a number with a unit:
//!
//! - `px`: pixels (the unit a bare number has);
//! - `em`: times the size of the element's font;
//! - `lh`: times a line of the element's font, its ascent and descent, which
//!   is what Fluxbox sizes a bar by.
//!
//! So `"1.5em"` is room that follows the text size, and `"1lh + 2 + 6"` is a
//! line of text with 2 pixels and a 3 pixel bevel at each end: the rule
//! Fluxbox sizes a titlebar by, written where it can be changed.

use std::fmt;

use serde::Deserialize as _;
use serde::de::{self, Deserializer, Visitor};

/// A length in a style file, until the font it may be relative to is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Length {
    /// Logical pixels.
    pixels: i32,
    /// Thousandths of the element's font size.
    em: i32,
    /// Thousandths of a line of the element's font.
    line: i32,
}

impl Length {
    /// Returns a length of `pixels` pixels.
    #[must_use]
    pub const fn pixels(pixels: u32) -> Self {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "a style's lengths are far below i32::MAX"
        )]
        let pixels = pixels as i32;
        Self {
            pixels,
            em: 0,
            line: 0,
        }
    }

    /// Returns whether the length depends on the height of a line of the
    /// font, which is measured, so it is not worked out unless it must be.
    #[must_use]
    pub const fn uses_line(self) -> bool {
        self.line != 0
    }

    /// Returns the length in pixels, where an em is `em` pixels (the size
    /// of the element's font) and a line is `line` pixels (its height),
    /// rounded to the nearest, and never less than nothing.
    #[must_use]
    pub fn to_pixels(self, em: f32, line: f32) -> u32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a style's lengths are small, far below 2^24"
        )]
        let total =
            self.pixels as f32 + self.em as f32 / 1000.0 * em + self.line as f32 / 1000.0 * line;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the total is rounded, small, and not negative"
        )]
        let pixels = total.round().max(0.0) as u32;
        pixels
    }

    /// Reads a length written as text: terms such as `12`, `12px`, `1.5em`
    /// or `1lh`, added and subtracted.
    fn parse(text: &str) -> Option<Self> {
        let mut rest = text.trim();
        let mut total = Self::default();
        let mut sign = 1_i32;
        let mut is_first = true;
        loop {
            let number_end = rest
                .find(|character: char| !character.is_ascii_digit() && character != '.')
                .unwrap_or(rest.len());
            let (number, after) = rest.split_at(number_end);
            let number: f64 = number.parse().ok()?;
            let after = after.trim_start();
            let unit_end = after
                .find(|character: char| !character.is_ascii_alphabetic())
                .unwrap_or(after.len());
            let (unit, after) = after.split_at(unit_end);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a length is small, far below i32::MAX"
            )]
            let thousandths = (number * 1000.0).round() as i32;
            match unit {
                "" | "px" => total.pixels += sign * (thousandths + 500).div_euclid(1000),
                "em" => total.em += sign * thousandths,
                "lh" => total.line += sign * thousandths,
                _ => return None,
            }
            let _ = is_first;
            is_first = false;
            let after = after.trim_start();
            let mut characters = after.chars();
            match characters.next() {
                None => return Some(total),
                Some('+') => sign = 1,
                Some('-') => sign = -1,
                Some(_) => return None,
            }
            rest = characters.as_str().trim_start();
        }
    }
}

impl<'de> serde::Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a number, or text such as `1.5em` or `1lh + 2`.
        struct LengthVisitor;

        impl Visitor<'_> for LengthVisitor {
            type Value = Length;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a length: a number of pixels, or text of terms added or subtracted, each a \
                     number with a unit (`px`, `em` times the font's size, `lh` times a line of \
                     it), such as `1.5em` or `1lh + 2`",
                )
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<Length, E> {
                u32::try_from(number)
                    .map(Length::pixels)
                    .map_err(|_out_of_range| {
                        E::invalid_value(de::Unexpected::Signed(number), &self)
                    })
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<Length, E> {
                u32::try_from(number)
                    .map(Length::pixels)
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

/// The four lengths round an element, top, right, bottom, left: written as
/// one length for all four, or a list of the four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sides(pub [Length; 4]);

impl<'de> serde::Deserialize<'de> for Sides {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a length, or a list of four.
        struct SidesVisitor;

        impl<'de> Visitor<'de> for SidesVisitor {
            type Value = Sides;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "one length for all four sides, or a list of four lengths (top, right, \
                     bottom, left); a length is a number of pixels, or text such as `1.5em` \
                     or `1lh + 2`",
                )
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<Sides, E> {
                let length = Length::deserialize(de::value::I64Deserializer::<E>::new(number))?;
                Ok(Sides([length; 4]))
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<Sides, E> {
                let length = Length::deserialize(de::value::U64Deserializer::<E>::new(number))?;
                Ok(Sides([length; 4]))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<Sides, E> {
                let length = Length::deserialize(de::value::StrDeserializer::<E>::new(text))?;
                Ok(Sides([length; 4]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Sides, A::Error> {
                let mut sides = [Length::default(); 4];
                for (index, side) in sides.iter_mut().enumerate() {
                    *side = seq
                        .next_element()?
                        .ok_or_else(|| de::Error::invalid_length(index, &self))?;
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(5, &self));
                }
                Ok(Sides(sides))
            }
        }

        deserializer.deserialize_any(SidesVisitor)
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
    #[case("4", 4, 0.0, 0.0)]
    #[case("\"4\"", 4, 0.0, 0.0)]
    #[case("\"12px\"", 12, 0.0, 0.0)]
    #[case("\"1.5em\"", 0, 1.5, 0.0)]
    #[case("\"0.25 em\"", 0, 0.25, 0.0)]
    #[case("\"1lh\"", 0, 0.0, 1.0)]
    #[case("\"1lh + 2 + 6\"", 8, 0.0, 1.0)]
    #[case("\"1lh+2-1\"", 1, 0.0, 1.0)]
    #[case("\"1.5em + 0.5lh - 3px\"", -3, 1.5, 0.5)]
    #[case("0", 0, 0.0, 0.0)]
    fn lengths_are_sums_of_pixels_ems_and_lines(
        #[case] text: &str,
        #[case] pixels: i32,
        #[case] em: f64,
        #[case] line: f64,
    ) {
        let length = read(text).expect("valid");
        // With an em of 10 pixels and a line of 20, 1000s of each are 10 and 20.
        let expected = f64::from(pixels) + em * 10.0 + line * 20.0;
        let got = f64::from(length.to_pixels(10.0, 20.0));
        assert!(
            (got - expected.max(0.0)).abs() < 0.51,
            "{text}: {got} vs {expected}"
        );
    }

    #[rstest]
    #[case("-3")]
    #[case("\"-1em\"")]
    #[case("\"1.5rem\"")]
    #[case("\"em\"")]
    #[case("\"1lh +\"")]
    #[case("\"1lh 2\"")]
    #[case("\"\"")]
    #[case("1.5")]
    #[case("true")]
    fn anything_else_is_refused_and_says_what_a_length_is(#[case] text: &str) {
        let error = read(text).expect_err(text).to_string();
        assert!(error.contains("a length"), "{error}");
    }

    #[test]
    fn a_length_is_rounded_to_a_pixel_and_never_below_nothing() {
        assert_eq!(read("\"1em\"").expect("valid").to_pixels(13.0, 0.0), 13);
        assert_eq!(
            read("\"0.5em\"").expect("valid").to_pixels(13.0, 0.0),
            7,
            "6.5 rounds up"
        );
        assert_eq!(
            read("5").expect("valid").to_pixels(13.0, 0.0),
            5,
            "pixels ignore the font"
        );
        assert_eq!(
            read("\"1lh - 30\"").expect("valid").to_pixels(10.0, 20.0),
            0,
            "less than nothing is nothing"
        );
    }

    #[test]
    fn only_a_length_with_a_line_needs_one_measured() {
        assert!(read("\"1lh + 2\"").expect("valid").uses_line());
        assert!(!read("\"1.5em\"").expect("valid").uses_line());
        assert!(!read("4").expect("valid").uses_line());
    }
}
