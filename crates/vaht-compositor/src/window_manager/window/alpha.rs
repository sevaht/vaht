//! How opaque a window is, and `set-alpha`'s changes to it (Fluxbox's
//! `SetAlpha`).

use std::fmt;

/// How opaque a window is when focused and when not: 0 is invisible, 255
/// opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WindowAlpha {
    /// While it has keyboard focus.
    pub(crate) focused: u8,
    /// While it does not.
    pub(crate) unfocused: u8,
}

impl WindowAlpha {
    /// Returns the opacity for a window that is `is_focused`, as the
    /// renderer takes it: 0.0 to 1.0.
    pub(crate) fn opacity(self, is_focused: bool) -> f32 {
        let alpha = if is_focused {
            self.focused
        } else {
            self.unfocused
        };
        f32::from(alpha) / 255.0
    }
}

/// What `set-alpha` does to a window's opacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlphaChange {
    /// Back to the settings' (`set-alpha` alone).
    Reset,
    /// Each opacity changed (`set-alpha <both>`, or `set-alpha <focused>
    /// <unfocused>`).
    Set {
        /// The focused opacity's change.
        focused: AlphaValue,
        /// The unfocused opacity's change.
        unfocused: AlphaValue,
    },
}

impl AlphaChange {
    /// Sets both opacities to `alpha`.
    pub(crate) const fn both(alpha: u8) -> Self {
        Self::Set {
            focused: AlphaValue::To(alpha),
            unfocused: AlphaValue::To(alpha),
        }
    }

    /// Parses `set-alpha`'s arguments: none, one for both, or the focused
    /// then the unfocused; each a number to 255, or a change by one with
    /// `+` or `-`.
    pub(crate) fn parse(arguments: &str) -> Result<Self, String> {
        let values: Vec<&str> = arguments.split_whitespace().collect();
        let value = |text: &str| {
            AlphaValue::parse(text).ok_or_else(|| {
                format!("`set-alpha` takes numbers to 255, or changes like +10, not `{text}`")
            })
        };
        match values.as_slice() {
            [] => Ok(Self::Reset),
            [both] => {
                let both = value(both)?;
                Ok(Self::Set {
                    focused: both,
                    unfocused: both,
                })
            }
            [focused, unfocused] => Ok(Self::Set {
                focused: value(focused)?,
                unfocused: value(unfocused)?,
            }),
            [_, _, _, ..] => Err("`set-alpha` takes at most two values".to_owned()),
        }
    }

    /// Returns the window's opacity after the change, from `current`; none
    /// to go back to the settings'.
    pub(crate) fn applied(self, current: WindowAlpha) -> Option<WindowAlpha> {
        match self {
            Self::Reset => None,
            Self::Set { focused, unfocused } => Some(WindowAlpha {
                focused: focused.applied(current.focused),
                unfocused: unfocused.applied(current.unfocused),
            }),
        }
    }
}

impl fmt::Display for AlphaChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reset => formatter.write_str("set-alpha"),
            Self::Set { focused, unfocused } if focused == unfocused => {
                write!(formatter, "set-alpha {focused}")
            }
            Self::Set { focused, unfocused } => {
                write!(formatter, "set-alpha {focused} {unfocused}")
            }
        }
    }
}

/// One opacity's change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlphaValue {
    /// To this opacity.
    To(u8),
    /// By this much, within 0 and 255.
    By(i16),
}

impl AlphaValue {
    /// Parses `200`, `+10` or `-10`.
    fn parse(text: &str) -> Option<Self> {
        if text.starts_with(['+', '-']) {
            text.parse().ok().map(Self::By)
        } else {
            text.parse().ok().map(Self::To)
        }
    }

    /// Returns `current` changed.
    fn applied(self, current: u8) -> u8 {
        match self {
            Self::To(alpha) => alpha,
            Self::By(change) => {
                let changed = (i16::from(current) + change).clamp(0, 255);
                u8::try_from(changed).expect("clamped to a byte")
            }
        }
    }
}

impl fmt::Display for AlphaValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::To(alpha) => write!(formatter, "{alpha}"),
            Self::By(change) => write!(formatter, "{change:+}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half opaque either way.
    const HALF: WindowAlpha = WindowAlpha {
        focused: 128,
        unfocused: 128,
    };

    #[test]
    fn one_value_sets_both_and_two_set_each() {
        let both = AlphaChange::parse("200").expect("valid");
        assert_eq!(
            both.applied(HALF),
            Some(WindowAlpha {
                focused: 200,
                unfocused: 200
            })
        );
        let each = AlphaChange::parse("255 +10").expect("valid");
        assert_eq!(
            each.applied(HALF),
            Some(WindowAlpha {
                focused: 255,
                unfocused: 138
            })
        );
        assert_eq!(each.to_string(), "set-alpha 255 +10");
    }

    #[test]
    fn changes_stay_within_a_byte_and_nothing_resets() {
        let change = AlphaChange::parse("-200").expect("valid");
        assert_eq!(change.applied(HALF).map(|alpha| alpha.focused), Some(0));
        assert_eq!(AlphaChange::parse("").expect("valid").applied(HALF), None);
        assert!(AlphaChange::parse("300").is_err());
        assert!(AlphaChange::parse("1 2 3").is_err());
    }
}
