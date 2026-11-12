//! Colors, as themes write them.

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

impl FromStr for Color {
    type Err = ParseColorError;

    /// Parses `#rrggbb` or `#rrggbbaa`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseColorError {
            text: text.to_owned(),
        };
        let digits = text.strip_prefix('#').ok_or_else(invalid)?;
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
#[error("not a color: {text:?} (expected #rrggbb or #rrggbbaa)")]
pub struct ParseColorError {
    /// The text as given.
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_parse_with_and_without_alpha() {
        assert_eq!("#8c8c8c".parse(), Ok(Color::rgb(140, 140, 140)));
        assert_eq!("#ffffff80".parse(), Ok(Color::rgba(255, 255, 255, 128)));
    }

    #[test]
    fn formatting_and_parsing_round_trip() {
        for color in [Color::rgb(1, 2, 3), Color::rgba(250, 128, 0, 7)] {
            assert_eq!(color.to_string().parse(), Ok(color));
        }
    }

    #[test]
    fn malformed_colors_are_rejected() {
        for text in ["8c8c8c", "#8c8c8", "#8c8c8cz", "#gggggg", ""] {
            assert!(text.parse::<Color>().is_err(), "{text:?} parsed");
        }
    }
}
