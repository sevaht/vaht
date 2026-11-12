//! Turning keyboard and pointer events into what clients receive.
//!
//! For now: keys go to the focused window, a click focuses and raises the
//! window under the pointer, and pointer events go to the surface under it.
//! Bindings, grabs and the rest of `Input` (01 §7) build on this.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::Compositor;

/// Scroll distance per wheel notch, in the 120ths that high-resolution
/// wheels report: a notch is 120, and libinput maps it to 15 units.
const SCROLL_UNITS_PER_V120: f64 = 15.0 / 120.0;

impl Compositor {
    /// Delivers one input event.
    pub(crate) fn handle_input<B: InputBackend>(&mut self, event: InputEvent<B>) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "Smithay's input enum has many kinds vaht does not handle yet"
        )]
        match event {
            InputEvent::Keyboard { event } => self.handle_key::<B>(&event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_position::<B>(&event);
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button::<B>(&event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis::<B>(&event),
            _ => {}
        }
    }

    /// Sends a key to the focused window.
    fn handle_key<B: InputBackend>(&mut self, event: &B::KeyboardKeyEvent) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.input::<(), _>(
            self,
            event.key_code(),
            event.state(),
            SERIAL_COUNTER.next_serial(),
            event.time(),
            |_, _, _| FilterResult::Forward,
        );
    }

    /// Moves the pointer to an absolute position on the output.
    fn handle_pointer_position<B: InputBackend>(&mut self, event: &B::PointerMotionAbsoluteEvent) {
        let output = self.backend.output_geometry();
        let location = event.position_transformed(output.size) + output.loc.to_f64();
        let under = self
            .windows
            .surface_under(location)
            .map(|(_, surface, origin)| (surface, origin));
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    /// Presses or releases a pointer button; a press focuses and raises the
    /// window under the pointer.
    fn handle_pointer_button<B: InputBackend>(&mut self, event: &B::PointerButtonEvent) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let serial = SERIAL_COUNTER.next_serial();
        if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
            let clicked = self
                .windows
                .surface_under(pointer.current_location())
                .map(|(id, _, _)| id);
            if let Some(id) = clicked {
                self.raise_window(id);
            }
            self.focus_window(clicked, serial);
        }
        pointer.button(
            self,
            &ButtonEvent {
                button: event.button_code(),
                state: event.state(),
                serial,
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    /// Scrolls the surface under the pointer.
    fn handle_pointer_axis<B: InputBackend>(&mut self, event: &B::PointerAxisEvent) {
        let source = event.source();
        let mut frame = AxisFrame::new(event.time()).source(source);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let discrete = event.amount_v120(axis);
            let amount = event
                .amount(axis)
                .unwrap_or_else(|| discrete.unwrap_or(0.0) * SCROLL_UNITS_PER_V120);
            if amount != 0.0 {
                frame = frame.value(axis, amount);
                if let Some(discrete) = discrete {
                    // Discrete steps are whole 120ths; the cast drops nothing.
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "v120 values are whole numbers"
                    )]
                    let steps = discrete as i32;
                    frame = frame.v120(axis, steps);
                }
            }
            // A finger lifted from a touchpad ends kinetic scrolling.
            if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                frame = frame.stop(axis);
            }
        }
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
