//! Colors, as styles write them: `#rrggbb`, `#rrggbbaa`, or an X11 color
//! name such as `grey55` or `SteelBlue`, as X.Org's `rgb.txt` names them.

mod x11;

use std::fmt;
use std::str::FromStr;

/// A color with straight (not premultiplied) alpha, eight bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    /// Red intensity.
    pub red: u8,
    /// Green intensity.
    pub green: u8,
    /// Blue intensity.
    pub blue: u8,
    /// Opacity: 0 is clear, 255 is solid.
    pub alpha: u8,
}

impl Color {
    /// Solid black.
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Solid white.
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    /// Fully clear.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// Returns an opaque color.
    #[must_use]
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::rgba(red, green, blue, u8::MAX)
    }

    /// Returns a color with the given opacity.
    #[must_use]
    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Returns the X11 color `name`, matched as `XParseColor` does: in any
    /// letter case, spaces ignored (`dark slate gray` is `DarkSlateGray`).
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        let key: String = name
            .chars()
            .filter(|character| *character != ' ')
            .map(|character| character.to_ascii_lowercase())
            .collect();
        x11::NAMED_COLORS
            .binary_search_by(|(known, _)| (*known).cmp(key.as_str()))
            .ok()
            .and_then(|index| x11::NAMED_COLORS.get(index))
            .map(|&(_, color)| color)
    }

    /// Returns the color as tiny-skia takes it.
    #[must_use]
    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.red, self.green, self.blue, self.alpha)
    }
}

impl fmt::Display for Color {
    /// Formats as `#rrggbb`, or `#rrggbbaa` when not opaque.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "#{:02x}{:02x}{:02x}",
            self.red, self.green, self.blue
        )?;
        if self.alpha != u8::MAX {
            write!(formatter, "{:02x}", self.alpha)?;
        }
        Ok(())
    }
}

impl Color {
    /// Reads X11's `rgb:R/G/B` (what follows `rgb:`): each channel one to four
    /// hex digits, scaled so all digits set is full intensity.
    fn x11_channels(channels: &str) -> Option<Self> {
        let mut parts = channels.split('/');
        let mut channel = || {
            let digits = parts.next()?;
            if !(1..=4).contains(&digits.len())
                || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let value = u32::from_str_radix(digits, 16).ok()?;
            let most = (1_u32 << (4 * u32::try_from(digits.len()).ok()?)) - 1;
            u8::try_from((value * 255 + most / 2) / most).ok()
        };
        let (red, green, blue) = (channel()?, channel()?, channel()?);
        parts
            .next()
            .is_none()
            .then(|| Self::rgba(red, green, blue, u8::MAX))
    }
}

impl FromStr for Color {
    type Err = ParseColorError;

    /// Parses `#rrggbb`, `#rrggbbaa`, X11's `#rgb` and `rgb:R/G/B`, or an X11
    /// color name.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseColorError {
            text: text.to_owned(),
        };
        if let Some(channels) = text.strip_prefix("rgb:") {
            return Self::x11_channels(channels).ok_or_else(invalid);
        }
        let Some(digits) = text.strip_prefix('#') else {
            return Self::named(text).ok_or_else(invalid);
        };
        if digits.len() == 3 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            // X11's `#rgb`: each digit is repeated, so `#fff` is white.
            let channel = |index: usize| {
                digits
                    .get(index..=index)
                    .and_then(|digit| u8::from_str_radix(digit, 16).ok())
                    .map(|value| value * 17)
            };
            return match (channel(0), channel(1), channel(2)) {
                (Some(red), Some(green), Some(blue)) => Ok(Self::rgba(red, green, blue, u8::MAX)),
                _ => Err(invalid()),
            };
        }
        if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let channel = |index: usize| {
            digits
                .get(index * 2..index * 2 + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        };
        let alpha = if digits.len() == 8 {
            channel(3)
        } else {
            Some(u8::MAX)
        };
        match (channel(0), channel(1), channel(2), alpha) {
            (Some(red), Some(green), Some(blue), Some(alpha)) => {
                Ok(Self::rgba(red, green, blue, alpha))
            }
            _ => Err(invalid()),
        }
    }
}

/// Text that is not a color vaht understands.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a color: {text:?} (expected #rrggbb, #rrggbbaa, rgb:R/G/B or an X11 color name)")]
pub struct ParseColorError {
    /// The text as given.
    pub text: String,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn hex_colors_parse_with_and_without_alpha() {
        assert_eq!("#8c8c8c".parse(), Ok(Color::rgb(140, 140, 140)));
        assert_eq!("#ffffff80".parse(), Ok(Color::rgba(255, 255, 255, 128)));
    }

    #[rstest]
    #[case("rgb:80/84/88", Color::rgb(0x80, 0x84, 0x88))]
    #[case("rgb:f/0/8", Color::rgb(255, 0, 136))]
    #[case("rgb:ffff/0000/8080", Color::rgb(255, 0, 128))]
    #[case("#fff", Color::rgb(255, 255, 255))]
    #[case("#123", Color::rgb(0x11, 0x22, 0x33))]
    fn x11_color_forms_parse(#[case] text: &str, #[case] color: Color) {
        assert_eq!(text.parse(), Ok(color));
    }

    #[rstest]
    #[case("rgb:80/84")]
    #[case("rgb:80/84/88/90")]
    #[case("rgb:80/84/zz")]
    #[case("rgb:/84/88")]
    #[case("rgb:12345/0/0")]
    fn bad_x11_forms_are_refused(#[case] text: &str) {
        assert!(text.parse::<Color>().is_err());
    }

    #[test]
    fn formatting_and_parsing_round_trip() {
        for color in [Color::rgb(1, 2, 3), Color::rgba(250, 128, 0, 7)] {
            assert_eq!(color.to_string().parse(), Ok(color));
        }
    }

    #[rstest]
    #[case::grey("grey55", Color::rgb(140, 140, 140))]
    #[case::mixed_case("SteelBlue", Color::rgb(70, 130, 180))]
    #[case::spaced("dark slate gray", Color::rgb(47, 79, 79))]
    #[case::first("alice blue", Color::rgb(240, 248, 255))]
    #[case::last("YellowGreen", Color::rgb(154, 205, 50))]
    fn x11_names_parse_as_xparsecolor_reads_them(#[case] name: &str, #[case] expected: Color) {
        assert_eq!(name.parse(), Ok(expected));
    }

    #[test]
    fn the_x11_table_is_sorted_for_searching() {
        assert!(
            x11::NAMED_COLORS
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0)
        );
    }

    #[test]
    fn malformed_colors_are_rejected() {
        for text in ["8c8c8c", "#8c8c8", "#8c8c8cz", "#gggggg", "", "greyish"] {
            assert!(text.parse::<Color>().is_err(), "{text:?} parsed");
        }
    }
}
