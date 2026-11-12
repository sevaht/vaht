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
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::Compositor;
use crate::window::{FramePart, TitlebarButton, TitlebarHit, Window, WindowId};
use crate::windows::WindowTarget;

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
        // Over a frame, no client has the pointer: the frame is vaht's.
        let under = match self.windows.target_at(location, self.theme.metrics()) {
            Some(WindowTarget::Surface {
                surface, origin, ..
            }) => Some((surface, origin)),
            Some(WindowTarget::Frame { .. }) | None => None,
        };
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

    /// Presses or releases a pointer button. A press focuses and raises the
    /// window under the pointer and, on its titlebar, holds the widget there
    /// down; the release completes a titlebar click.
    fn handle_pointer_button<B: InputBackend>(&mut self, event: &B::PointerButtonEvent) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let serial = SERIAL_COUNTER.next_serial();
        let location = pointer.current_location();
        match event.state() {
            ButtonState::Pressed if !pointer.is_grabbed() => {
                let target = self.windows.target_at(location, self.theme.metrics());
                let clicked = target.as_ref().map(WindowTarget::window);
                if let Some(id) = clicked {
                    self.raise_window(id);
                }
                self.focus_window(clicked, serial);
                if let Some(WindowTarget::Frame {
                    window,
                    part: FramePart::Titlebar(point),
                }) = target
                {
                    self.press_titlebar(window, point);
                }
            }
            ButtonState::Pressed => {}
            ButtonState::Released => self.release_titlebar(location),
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

    /// Holds down the titlebar widget at `point` of `window`'s titlebar.
    fn press_titlebar(&mut self, window: WindowId, point: vaht_ui::Point) {
        if let Some(frame) = self.windows.get_mut(window).map(Window::frame_mut) {
            frame.titlebar_mut().press(point);
            self.held_titlebar = Some(window);
            self.schedule_render();
        }
    }

    /// Lets go of a held titlebar widget at global `location`, acting on it
    /// if the pointer is still on it.
    fn release_titlebar(&mut self, location: Point<f64, Logical>) {
        let Some(id) = self.held_titlebar.take() else {
            return;
        };
        let metrics = self.theme.metrics();
        let Some(window) = self.windows.get_mut(id) else {
            return;
        };
        let titlebar = window.frame_geometry(metrics).titlebar;
        let inside = location.to_i32_floor() - titlebar.loc;
        let clicked = window
            .frame_mut()
            .titlebar_mut()
            .release(vaht_ui::Point::new(inside.x, inside.y));
        match clicked {
            Some(TitlebarHit::Button(TitlebarButton::Close)) => window.toplevel().close(),
            Some(TitlebarHit::Label) | None => {}
        }
        self.schedule_render();
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
