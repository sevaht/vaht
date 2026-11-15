//! What rules exempt a window from: being listed, being cycled to, its
//! client's size hints, and its client's asking to fill the screen.

/// One thing a rule can exempt a window from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exemption {
    /// Being listed on the taskbar (Fluxbox's `[IconHidden]`).
    WindowList,
    /// Being cycled to (`[FocusHidden]`).
    Cycling,
    /// The sizes its client accepts (`[IgnoreSizeHints]`): resizing by
    /// hand is not held to them.
    SizeHints,
    /// Its client's asking to maximize it.
    ClientMaximize,
    /// Its client's asking to make it fullscreen.
    ClientFullscreen,
}

impl Exemption {
    /// Returns its bit in a set.
    const fn bit(self) -> u8 {
        match self {
            Self::WindowList => 1,
            Self::Cycling => 1 << 1,
            Self::SizeHints => 1 << 2,
            Self::ClientMaximize => 1 << 3,
            Self::ClientFullscreen => 1 << 4,
        }
    }
}

/// A set of exemptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Exemptions(u8);

impl Exemptions {
    /// Returns whether `exemption` is in the set.
    pub(crate) const fn has(self, exemption: Exemption) -> bool {
        self.0 & exemption.bit() != 0
    }

    /// Returns the set with `exemption` in it if `is_exempt`, else without.
    #[must_use]
    pub(crate) const fn with(self, exemption: Exemption, is_exempt: bool) -> Self {
        if is_exempt {
            Self(self.0 | exemption.bit())
        } else {
            Self(self.0 & !exemption.bit())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemptions_are_kept_apart() {
        let set = Exemptions::default()
            .with(Exemption::Cycling, true)
            .with(Exemption::ClientFullscreen, true)
            .with(Exemption::Cycling, false);
        assert!(set.has(Exemption::ClientFullscreen));
        assert!(!set.has(Exemption::Cycling));
        assert!(!set.has(Exemption::WindowList));
    }
}
