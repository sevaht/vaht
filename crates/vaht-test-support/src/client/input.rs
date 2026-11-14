//! The virtual pointer and keyboard a test client drives vaht with.
//!
//! [`VirtualInput`] only sends requests; the client waits for vaht to have
//! handled them, so input reaches vaht in order with everything else the
//! client sends on its one connection.

use std::fs::File;
use std::os::fd::AsFd as _;

use smithay_client_toolkit::reexports::client::QueueHandle;
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::{wl_pointer, wl_seat};
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::state::{ClientState, NoEvents, bind};

/// Linux input codes of a few keys (`KEY_*`), read with vaht's keymap.
pub mod keys {
    /// Escape.
    pub const ESCAPE: u32 = 1;
    /// Tab.
    pub const TAB: u32 = 15;
    /// The left Control key.
    pub const LEFT_CONTROL: u32 = 29;
    /// The left Shift key.
    pub const LEFT_SHIFT: u32 = 42;
    /// F1.
    pub const F1: u32 = 59;
    /// F2.
    pub const F2: u32 = 60;
    /// F11.
    pub const F11: u32 = 87;
    /// The up arrow.
    pub const UP: u32 = 103;
    /// The left arrow.
    pub const LEFT: u32 = 105;
    /// The right arrow.
    pub const RIGHT: u32 = 106;
    /// The down arrow.
    pub const DOWN: u32 = 108;
    /// The T key.
    pub const LETTER_T: u32 = 20;
    /// The left Alt key.
    pub const LEFT_ALT: u32 = 56;
    /// F4.
    pub const F4: u32 = 62;
    /// The left Super (Windows, logo) key.
    pub const LEFT_SUPER: u32 = 125;
    /// Return (Enter).
    pub const ENTER: u32 = 28;
    /// The space bar.
    pub const SPACE: u32 = 57;

    /// Returns the key that types `character` on a US keyboard, and
    /// whether Shift must be held: a letter, a digit, a space, `-`, `.` or
    /// `/`.
    #[must_use]
    pub fn typing(character: char) -> Option<(u32, bool)> {
        let key = unshifted(character.to_ascii_lowercase())?;
        Some((key, character.is_ascii_uppercase()))
    }

    /// Returns the key that types `character` without Shift.
    fn unshifted(character: char) -> Option<u32> {
        const ROWS: [(&str, u32); 4] = [
            ("1234567890-", 2),
            ("qwertyuiop", 16),
            ("asdfghjkl", 30),
            ("zxcvbnm,./", 44),
        ];
        if character == ' ' {
            return Some(SPACE);
        }
        ROWS.iter().find_map(|(row, first)| {
            let column = row.chars().position(|key| key == character)?;
            Some(first + u32::try_from(column).ok()?)
        })
    }
}

/// `wl_keyboard.key_state.released`.
const KEY_RELEASED: u32 = 0;

/// `wl_keyboard.key_state.pressed`.
const KEY_PRESSED: u32 = 1;

/// `wl_keyboard.keymap_format.no_keymap`: the keyboard uses the seat's keymap.
const NO_KEYMAP_FORMAT: u32 = 0;

/// The modifier mask of Alt in the default xkb keymap, where Alt is Mod1:
/// the fourth real modifier, after Shift, Lock and Control.
const ALT_MODIFIER_MASK: u32 = 1 << 3;

/// Whether a key goes down or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyMotion {
    /// It is pressed.
    Press,
    /// It is released.
    Release,
}

/// A virtual pointer and keyboard, and the clock their events carry.
#[derive(Debug)]
pub(super) struct VirtualInput {
    /// The pointer, which moves on the output.
    pointer: ZwlrVirtualPointerV1,
    /// The keyboard, which presses keys and holds modifiers.
    keyboard: ZwpVirtualKeyboardV1,
    /// The empty file sent as the keyboard's keymap; kept open until sent.
    _no_keymap: File,
    /// The milliseconds clock stamped on input events.
    time: u32,
}

impl VirtualInput {
    /// Creates a virtual pointer, and a virtual keyboard on `seat`.
    ///
    /// # Panics
    ///
    /// Panics if vaht does not offer virtual input.
    pub(super) fn bind(
        globals: &GlobalList,
        handle: &QueueHandle<ClientState>,
        seat: &wl_seat::WlSeat,
    ) -> Self {
        let pointer_manager: ZwlrVirtualPointerManagerV1 =
            bind(globals, handle, "zwlr_virtual_pointer_manager_v1");
        let pointer = pointer_manager.create_virtual_pointer(None, handle, NoEvents);
        let keyboard_manager: ZwpVirtualKeyboardManagerV1 =
            bind(globals, handle, "zwp_virtual_keyboard_manager_v1");
        let keyboard = keyboard_manager.create_virtual_keyboard(seat, handle, NoEvents);
        // `no_keymap`: the keyboard's modifier masks are read with vaht's own
        // keymap, so no keymap needs writing. The protocol still wants a file.
        let no_keymap = File::open("/dev/null").expect("/dev/null opens");
        keyboard.keymap(NO_KEYMAP_FORMAT, no_keymap.as_fd(), 0);
        Self {
            pointer,
            keyboard,
            _no_keymap: no_keymap,
            time: 0,
        }
    }

    /// Moves the pointer to `x`, `y` in an output layout of `layout_size`.
    pub(super) fn move_pointer_to(&mut self, x: u32, y: u32, layout_size: (u32, u32)) {
        let (width, height) = layout_size;
        let time = self.tick();
        self.pointer.motion_absolute(time, x, y, width, height);
        self.pointer.frame();
    }

    /// Moves the pointer by `delta`, as a mouse does.
    pub(super) fn move_pointer_by(&mut self, delta: (f64, f64)) {
        let time = self.tick();
        self.pointer.motion(time, delta.0, delta.1);
        self.pointer.frame();
    }

    /// Presses or releases `button`.
    pub(super) fn button(&mut self, button: u32, state: wl_pointer::ButtonState) {
        let time = self.tick();
        self.pointer.button(time, button, state);
        self.pointer.frame();
    }

    /// Presses or releases `key`.
    pub(super) fn key(&mut self, key: u32, motion: KeyMotion) {
        let time = self.tick();
        let state = match motion {
            KeyMotion::Press => KEY_PRESSED,
            KeyMotion::Release => KEY_RELEASED,
        };
        self.keyboard.key(time, key, state);
    }

    /// Holds Alt down.
    pub(super) fn hold_alt(&self) {
        self.keyboard.modifiers(ALT_MODIFIER_MASK, 0, 0, 0);
    }

    /// Lets go of every held modifier.
    pub(super) fn release_modifiers(&self) {
        self.keyboard.modifiers(0, 0, 0, 0);
    }

    /// Returns the next input event time, a millisecond after the last.
    const fn tick(&mut self) -> u32 {
        self.time += 1;
        self.time
    }
}
