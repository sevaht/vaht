//! A font's size in points, written as a number or as a sum of them: the
//! rule a font choice of a style uses to add to the size the style's other
//! choices give (`"$title_font_size + $font_bump"`).

use std::fmt;

use serde::de::{self, Deserializer, Visitor};

/// A font size in points: at least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointSize(u16);

impl PointSize {
    /// Returns the size in points.
    #[must_use]
    pub const fn points(self) -> u16 {
        self.0
    }

    /// Reads a size written as text: whole numbers added and subtracted
    /// (`8 + 2`, `10 - 1`). A total below 1 is 1.
    fn parse(text: &str) -> Option<Self> {
        let mut rest = text.trim();
        let mut total = 0_i64;
        let mut sign = 1_i64;
        loop {
            let end = rest
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(rest.len());
            let (number, after) = rest.split_at(end);
            total += sign * number.parse::<i64>().ok()?;
            let mut characters = after.trim_start().chars();
            match characters.next() {
                None => break,
                Some('+') => sign = 1,
                Some('-') => sign = -1,
                Some(_) => return None,
            }
            rest = characters.as_str().trim_start();
        }
        u16::try_from(total.max(1)).ok().map(Self)
    }
}

impl<'de> serde::Deserialize<'de> for PointSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a number, or text such as `8 + 2`.
        struct PointSizeVisitor;

        impl Visitor<'_> for PointSizeVisitor {
            type Value = PointSize;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a size in points: a whole number, or text of whole numbers added or \
                     subtracted, such as `8 + 2`",
                )
            }

            fn visit_i64<E: de::Error>(self, number: i64) -> Result<PointSize, E> {
                u16::try_from(number)
                    .ok()
                    .filter(|points| *points >= 1)
                    .map(PointSize)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Signed(number), &self))
            }

            fn visit_u64<E: de::Error>(self, number: u64) -> Result<PointSize, E> {
                u16::try_from(number)
                    .ok()
                    .filter(|points| *points >= 1)
                    .map(PointSize)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Unsigned(number), &self))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<PointSize, E> {
                PointSize::parse(text)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
            }
        }

        deserializer.deserialize_any(PointSizeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<u16, toml::de::Error> {
        toml::from_str::<toml::Table>(&format!("size = {text}"))
            .and_then(|table| table["size"].clone().try_into::<PointSize>())
            .map(PointSize::points)
    }

    #[test]
    fn a_number_is_its_points() {
        assert_eq!(read("8"), Ok(8));
        assert!(read("0").is_err(), "a font of no size is not one");
        assert!(read("-3").is_err());
    }

    #[test]
    fn text_adds_and_subtracts_whole_numbers() {
        assert_eq!(read("\"8 + 2\""), Ok(10));
        assert_eq!(read("\"10-1+1\""), Ok(10));
        assert_eq!(read("\" 9 \""), Ok(9));
        assert_eq!(read("\"3 - 7\""), Ok(1), "the least size is 1");
    }

    #[test]
    fn text_that_is_not_a_sum_of_whole_numbers_is_refused() {
        for text in [
            "\"\"", "\"8px\"", "\"1.5\"", "\"8 +\"", "\"+ 8\"", "\"two\"",
        ] {
            assert!(read(text).is_err(), "{text}");
        }
    }
}
