//! What a command runs on: the window, and for a mouse binding the press
//! and the grab it runs in (01 §8).
//!
//! Key bindings, mouse bindings, titlebar buttons and later menus and IPC
//! all hand [`crate::Compositor::execute`] a parsed command and an
//! [`ExecutionContext`]; nothing dispatches on strings.

use smithay::input::pointer::GrabStartData;
use smithay::utils::Serial;

use crate::Compositor;
use crate::grab::GrabKind;
use crate::window_manager::WindowId;

/// The mouse button press a command runs for.
#[derive(Debug, Clone)]
pub(crate) struct Press {
    /// Where, and with which button.
    pub(crate) start: GrabStartData<Compositor>,
    /// The press's serial.
    pub(crate) serial: Serial,
}

/// What a command acts on.
#[derive(Debug)]
pub(crate) struct ExecutionContext<'grab> {
    /// The window: the one clicked for a mouse binding, the focused one for
    /// a key binding.
    pub(crate) window: Option<WindowId>,
    /// The button press, for a mouse binding; `start-moving` and
    /// `start-resizing` need one.
    pub(crate) press: Option<Press>,
    /// The pointer grab a drag binding runs in: `start-moving` and
    /// `start-resizing` turn it into a move or resize instead of starting a
    /// new grab, which cannot be done from inside one.
    pub(crate) grab: Option<&'grab mut GrabKind>,
}

impl ExecutionContext<'_> {
    /// A context with only a window, as for a key binding.
    pub(crate) const fn for_window(window: Option<WindowId>) -> Self {
        Self {
            window,
            press: None,
            grab: None,
        }
    }

    /// A context for a mouse button press on `window`, or the desktop.
    pub(crate) const fn for_press(window: Option<WindowId>, press: Press) -> Self {
        Self {
            window,
            press: Some(press),
            grab: None,
        }
    }
}
