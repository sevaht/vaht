//! The keys and buttons an emulating program holds down, so they are let
//! go when it stops: a program that quits with a key held must not leave
//! it held.

/// The keys and buttons held down, as evdev codes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeldInputs {
    /// The keys held.
    keys: Vec<u32>,
    /// The buttons held.
    buttons: Vec<u32>,
}

impl HeldInputs {
    /// Notes that `key` went down (`is_pressed`) or up.
    pub(crate) fn key(&mut self, key: u32, is_pressed: bool) {
        Self::note(&mut self.keys, key, is_pressed);
    }

    /// Notes that `button` went down (`is_pressed`) or up.
    pub(crate) fn button(&mut self, button: u32, is_pressed: bool) {
        Self::note(&mut self.buttons, button, is_pressed);
    }

    /// Returns the keys and the buttons held, forgetting them: they are
    /// being let go.
    pub(crate) fn release_all(&mut self) -> (Vec<u32>, Vec<u32>) {
        (
            std::mem::take(&mut self.keys),
            std::mem::take(&mut self.buttons),
        )
    }

    /// Notes in `held` that `code` went down or up.
    fn note(held: &mut Vec<u32>, code: u32, is_pressed: bool) {
        held.retain(|known| *known != code);
        if is_pressed {
            held.push(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_still_down_is_let_go() {
        let mut held = HeldInputs::default();
        held.key(30, true);
        held.key(31, true);
        held.key(30, false);
        held.button(272, true);
        held.button(272, true);
        assert_eq!(held.release_all(), (vec![31], vec![272]));
        assert_eq!(held.release_all(), (vec![], vec![]), "let go once");
    }
}
