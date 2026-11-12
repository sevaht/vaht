//! Input from programs: `zwlr_virtual_pointer_manager_v1` and
//! `zwp_virtual_keyboard_manager_v1`.
//!
//! Automation tools (`wtype`, `wlrctl`), remote desktops and vaht's own UI
//! tests drive vaht through these. Their events become ordinary input events
//! and go through the same handling as a real mouse and keyboard, so
//! bindings, focus and grabs treat them alike.

use smithay::backend::input::InputEvent;
use smithay::wayland::virtual_keyboard::{
    VirtualKeyboardBackend, VirtualKeyboardHandler, VirtualKeyboardSpecialEvent,
};
use smithay::wayland::virtual_pointer::{VirtualPointerBackend, VirtualPointerHandler};

use crate::Compositor;

impl VirtualPointerHandler for Compositor {
    fn process_virtual_pointer_event(&mut self, event: InputEvent<VirtualPointerBackend>) {
        self.handle_input(event);
    }
}

impl VirtualKeyboardHandler for Compositor {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "a virtual keyboard sends only keys, its device's arrival and removal, and specials"
        )]
        match event {
            InputEvent::Keyboard { event } => self.handle_virtual_key(&event),
            InputEvent::DeviceRemoved { device } => self.release_virtual_keys(device.source()),
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
                device,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            }) => self.set_virtual_modifiers(
                device.keymap(),
                [mods_depressed, mods_latched, mods_locked, group],
            ),
            // A replaced keymap is activated with the device's next key.
            _ => {}
        }
    }
}
