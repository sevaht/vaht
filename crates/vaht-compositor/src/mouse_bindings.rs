//! What a mouse button press on a window does when modifiers are held.
//!
//! A bound press is vaht's: it starts a drag instead of reaching the client.
//! Bindings will come from `bindings.toml` (02 §6); until the configuration
//! files exist, the built-in table holds the window bindings of the Fluxbox
//! setup vaht replaces (`OnWindow Mod1 Mouse1 :StartMoving`,
//! `OnWindow Mod1 Mouse3 :StartResizing NearestCorner`).

use smithay::input::keyboard::ModifiersState;

/// The Linux input code of the left mouse button (`BTN_LEFT`).
const BUTTON_LEFT: u32 = 0x110;

/// The Linux input code of the right mouse button (`BTN_RIGHT`).
const BUTTON_RIGHT: u32 = 0x111;

/// What a bound press does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseAction {
    /// Raises, focuses and starts moving the window.
    StartMoving,
    /// Raises, focuses and starts resizing the window from the corner
    /// nearest the pointer.
    StartResizingNearestCorner,
}

/// A set of modifier keys, one bit each; a binding needs exactly its set
/// held, all others up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Modifiers(u8);

impl Modifiers {
    /// Alt (Mod1).
    const ALT: Self = Self(1);
    /// Control.
    const CONTROL: Self = Self(1 << 1);
    /// Shift.
    const SHIFT: Self = Self(1 << 2);
    /// Super (Mod4).
    const SUPER: Self = Self(1 << 3);

    /// Returns the modifiers held in `state`.
    fn held(state: &ModifiersState) -> Self {
        let bit = |is_held: bool, modifier: Self| if is_held { modifier.0 } else { 0 };
        Self(
            bit(state.alt, Self::ALT)
                | bit(state.ctrl, Self::CONTROL)
                | bit(state.shift, Self::SHIFT)
                | bit(state.logo, Self::SUPER),
        )
    }
}

/// A button, with modifiers, and what pressing it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseBinding {
    /// The modifiers that must be held.
    modifiers: Modifiers,
    /// The button, as a Linux input code.
    button: u32,
    /// What it does.
    action: MouseAction,
}

/// The built-in bindings for presses on a window.
const WINDOW_BINDINGS: [MouseBinding; 2] = [
    MouseBinding {
        modifiers: Modifiers::ALT,
        button: BUTTON_LEFT,
        action: MouseAction::StartMoving,
    },
    MouseBinding {
        modifiers: Modifiers::ALT,
        button: BUTTON_RIGHT,
        action: MouseAction::StartResizingNearestCorner,
    },
];

/// Returns what pressing `button` on a window does with `modifiers` held,
/// if anything; unbound presses go to the client.
pub(crate) fn window_action(modifiers: &ModifiersState, button: u32) -> Option<MouseAction> {
    let held = Modifiers::held(modifiers);
    WINDOW_BINDINGS
        .iter()
        .find(|binding| binding.modifiers == held && binding.button == button)
        .map(|binding| binding.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A modifier state with only Alt, and optionally Shift, held.
    fn alt(with_shift: bool) -> ModifiersState {
        ModifiersState {
            alt: true,
            shift: with_shift,
            ..ModifiersState::default()
        }
    }

    #[test]
    fn alt_left_moves_and_alt_right_resizes() {
        assert_eq!(
            window_action(&alt(false), BUTTON_LEFT),
            Some(MouseAction::StartMoving)
        );
        assert_eq!(
            window_action(&alt(false), BUTTON_RIGHT),
            Some(MouseAction::StartResizingNearestCorner)
        );
    }

    #[test]
    fn extra_or_missing_modifiers_leave_the_press_to_the_client() {
        assert_eq!(
            window_action(&alt(true), BUTTON_LEFT),
            None,
            "Alt+Shift is not Alt"
        );
        assert_eq!(window_action(&ModifiersState::default(), BUTTON_LEFT), None);
    }
}
