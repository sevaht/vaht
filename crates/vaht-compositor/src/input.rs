//! Turning keyboard and pointer events into bindings and what clients
//! receive.
//!
//! A key press is first matched against the key bindings; a bound press,
//! and its release, are vaht's and never reach the client. A button press
//! focuses and raises the window under the pointer, then is matched against
//! the mouse bindings of where it landed (01 §7): a bound press is held by a
//! pointer grab, so the client never sees it either. Unbound input goes to
//! the focused or hovered client.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::input::keyboard::{
    FilterResult, KeyboardSource, Keycode, KeymapFile, KeysymHandle, ModifiersState,
};
use smithay::input::pointer::{AxisFrame, ButtonEvent, GrabStartData, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Serial};
use smithay::wayland::virtual_keyboard::VirtualKeyboardKeyEvent;

use crate::Compositor;
use crate::config::{Command, KeyChord, Modifiers, MouseButton, MouseContext};
use crate::event_source::RepeatingTimer;
use crate::execute::{ExecutionContext, Press};
use crate::grab::GrabKind;
use crate::window::{FramePart, TitlebarButton, TitlebarHit, WindowId};
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
            InputEvent::PointerMotion { event } => self.handle_pointer_motion::<B>(&event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_position::<B>(&event);
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button::<B>(&event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis::<B>(&event),
            _ => {}
        }
    }

    /// Sends a key from a physical keyboard to the focused window.
    fn handle_key<B: InputBackend>(&mut self, event: &B::KeyboardKeyEvent) {
        self.activate_keymap(None);
        self.press_key(
            KeyboardSource::Physical,
            event.key_code(),
            event.state(),
            event.time(),
        );
    }

    /// Sends a key from a virtual keyboard, read with that keyboard's own
    /// keymap (or the seat's, when it sent none).
    pub(crate) fn handle_virtual_key(&mut self, event: &VirtualKeyboardKeyEvent) {
        let device = event.device();
        self.activate_keymap(device.keymap());
        self.press_key(
            device.source(),
            event.key_code(),
            event.state(),
            event.time(),
        );
    }

    /// Releases every key a virtual keyboard still holds; it is going away.
    pub(crate) fn release_virtual_keys(&mut self, source: KeyboardSource) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.release_source(self, source);
    }

    /// Sets the modifier state a virtual keyboard asks for: `masks` are its
    /// depressed, latched and locked modifiers and its layout group, relative
    /// to its `keymap` (or the seat's).
    pub(crate) fn set_virtual_modifiers(&mut self, keymap: Option<KeymapFile>, masks: [u32; 4]) {
        self.activate_keymap(keymap);
        let [depressed, latched, locked, group] = masks;
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        keyboard.with_xkb_state(self, |mut context| {
            context.set_modifier_mask(depressed, latched, locked, group);
        });
    }

    /// Makes `keymap` the seat's keymap, or vaht's own for `None`, if it is
    /// not already: a virtual keyboard's keycodes mean something only in its
    /// own keymap.
    fn activate_keymap(&mut self, keymap: Option<KeymapFile>) {
        let wanted = keymap.unwrap_or_else(|| self.physical_keymap.clone());
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        if keyboard.keymap_file().id() != wanted.id()
            && let Err(error) = keyboard.set_keymap_from_keymap_file(self, wanted)
        {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not switch keymaps"
            );
        }
    }

    /// Sends a key from `source` to the focused window, unless it is bound.
    fn press_key(
        &mut self,
        source: KeyboardSource,
        keycode: Keycode,
        state: KeyState,
        time: InputTime,
    ) {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        let bound = keyboard.input_from_source(
            source,
            self,
            keycode,
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |compositor, modifiers, keysym| {
                compositor.filter_key(keycode, state, modifiers, &keysym)
            },
        );
        if let Some(Some(command)) = bound {
            self.run_key_binding(keycode, command);
        }
    }

    /// Decides whether a key event is vaht's: a bound press, or the release
    /// of one. Returns the bound press's command.
    fn filter_key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        modifiers: &ModifiersState,
        keysym: &KeysymHandle<'_>,
    ) -> FilterResult<Option<Command>> {
        match state {
            KeyState::Pressed => {
                // Any new press ends a repeat, as with ordinary key repeat.
                self.input.key_repeat = None;
                // A fresh press of a key whose release was never seen.
                self.input.suppressed_keys.remove(&keycode);
                let Some(key) = keysym.raw_latin_sym_or_raw_current_sym() else {
                    return FilterResult::Forward;
                };
                let chord = KeyChord::new(Modifiers::held(modifiers), key);
                match self.config.bindings.key(chord) {
                    Some(command) => {
                        tracing::debug!(%chord, "key binding");
                        self.input.suppressed_keys.insert(keycode);
                        FilterResult::Intercept(Some(command.clone()))
                    }
                    None => FilterResult::Forward,
                }
            }
            KeyState::Released => {
                if self
                    .input
                    .key_repeat
                    .as_ref()
                    .is_some_and(|repeat| repeat.keycode == keycode)
                {
                    self.input.key_repeat = None;
                }
                if self.input.suppressed_keys.remove(&keycode) {
                    FilterResult::Intercept(None)
                } else {
                    FilterResult::Forward
                }
            }
        }
    }

    /// Runs a bound key's `command` for the focused window, and starts
    /// repeating it while `keycode` is held if it repeats.
    fn run_key_binding(&mut self, keycode: Keycode, command: Command) {
        self.execute(&command, &mut ExecutionContext::for_window(self.focused()));
        let Command::Repeat { rate, .. } = &command else {
            return;
        };
        let rate = rate.unwrap_or(Self::KEY_REPEAT_RATE);
        let interval = Duration::from_secs(1) / rate.get();
        let started = RepeatingTimer::start(
            &self.loop_handle,
            &self.ledger,
            Self::KEY_REPEAT_DELAY,
            interval,
            move |compositor: &mut Self| {
                let window = compositor.focused();
                compositor.execute(&command, &mut ExecutionContext::for_window(window));
            },
        );
        match started {
            Ok(timer) => {
                self.input.key_repeat = Some(KeyRepeat {
                    keycode,
                    _timer: timer,
                });
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not repeat a key binding"
            ),
        }
    }

    /// Returns the modifiers held now.
    fn held_modifiers(&self) -> Modifiers {
        let keyboard = self
            .seat
            .get_keyboard()
            .expect("the seat gets a keyboard at startup");
        Modifiers::held(&keyboard.modifier_state())
    }

    /// Moves the pointer by a relative distance, kept on the output.
    fn handle_pointer_motion<B: InputBackend>(&mut self, event: &B::PointerMotionEvent) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let output = self.backend.output_geometry().to_f64();
        let moved = pointer.current_location() + event.delta();
        // The far edges are outside the output, so stop just short of them.
        let location = Point::from((
            moved
                .x
                .clamp(output.loc.x, output.loc.x + output.size.w - 1.0),
            moved
                .y
                .clamp(output.loc.y, output.loc.y + output.size.h - 1.0),
        ));
        self.move_pointer_to(location, event.time());
    }

    /// Moves the pointer to an absolute position on the output.
    fn handle_pointer_position<B: InputBackend>(&mut self, event: &B::PointerMotionAbsoluteEvent) {
        let output = self.backend.output_geometry();
        let location = event.position_transformed(output.size) + output.loc.to_f64();
        self.move_pointer_to(location, event.time());
    }

    /// Moves the pointer to `location`, telling the surface under it.
    fn move_pointer_to(&mut self, location: Point<f64, Logical>, time: InputTime) {
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
                time,
            },
        );
        pointer.frame(self);
    }

    /// Presses or releases a pointer button.
    fn handle_pointer_button<B: InputBackend>(&mut self, event: &B::PointerButtonEvent) {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let serial = SERIAL_COUNTER.next_serial();
        let location = pointer.current_location();
        match event.state() {
            ButtonState::Pressed if !pointer.is_grabbed() => {
                self.press_button(event.button_code(), location, serial);
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

    /// Handles a press of `button` at `location` while nothing holds the
    /// pointer: focuses and raises the window there, then runs what the
    /// press is bound to. A titlebar button pressed without modifiers is
    /// held down until released; an unbound press on a border resizes.
    fn press_button(&mut self, button: u32, location: Point<f64, Logical>, serial: Serial) {
        let target = self.windows.target_at(location, self.theme.metrics());
        let clicked = target.as_ref().map(WindowTarget::window);
        if let Some(id) = clicked {
            self.raise_window(id);
        }
        self.focus_window(clicked, serial);
        let modifiers = self.held_modifiers();
        if let Some(WindowTarget::Frame {
            window,
            part: FramePart::Titlebar(point),
        }) = target
            && modifiers == Modifiers::default()
            && self.press_titlebar_button(window, point)
        {
            return;
        }
        let is_double = self.input.is_double_click(button, clicked);
        let bound = MouseButton::from_code(button).and_then(|mouse_button| {
            let bindings = self.config.bindings.button(
                contexts_of(target.as_ref()),
                modifiers,
                mouse_button,
                is_double,
            )?;
            Some((bindings.press.cloned(), bindings.drag.cloned()))
        });
        let press = Press {
            start: GrabStartData {
                focus: None,
                button,
                location,
            },
            serial,
        };
        match (bound, target) {
            (Some((on_press, on_drag)), _) => {
                if let Some(command) = on_press {
                    let mut context = ExecutionContext::for_press(clicked, press.clone());
                    self.execute(&command, &mut context);
                }
                let pointer = self
                    .seat
                    .get_pointer()
                    .expect("the seat gets a pointer at startup");
                // Unless the binding started a drag, hold the press, so it
                // never reaches a client.
                if !pointer.is_grabbed() {
                    self.hold_press(clicked, on_drag, press);
                }
            }
            (
                None,
                Some(WindowTarget::Frame {
                    window,
                    part: FramePart::Border(edges),
                }),
            ) => self.start_window_grab(window, GrabKind::Resize { edges }, press.start, serial),
            (None, Some(WindowTarget::Frame { .. } | WindowTarget::Surface { .. }) | None) => {}
        }
    }

    /// Holds down the titlebar button of `window` at `point`, if there is
    /// one; returns whether there was.
    fn press_titlebar_button(&mut self, window: WindowId, point: vaht_ui::Point) -> bool {
        let Some(target) = self.windows.get_mut(window) else {
            return false;
        };
        let titlebar = target.frame_mut().titlebar_mut();
        if !matches!(titlebar.hit_at(point), Some(TitlebarHit::Button(_))) {
            return false;
        }
        titlebar.press(point);
        self.held_titlebar = Some(window);
        self.schedule_render();
        true
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

    /// Scrolls the surface under the pointer, unless the wheel is bound
    /// there.
    fn handle_pointer_axis<B: InputBackend>(&mut self, event: &B::PointerAxisEvent) {
        if self.run_scroll_binding(event.amount_v120(Axis::Vertical)) {
            return;
        }
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

impl Compositor {
    /// Runs what a wheel turn of `steps` (in 120ths of a notch, positive
    /// down) is bound to where the pointer is; returns whether it was bound.
    fn run_scroll_binding(&mut self, steps: Option<f64>) -> bool {
        let pointer = self
            .seat
            .get_pointer()
            .expect("the seat gets a pointer at startup");
        let button = match steps {
            _ if pointer.is_grabbed() => return false,
            Some(steps) if steps < 0.0 => MouseButton::ScrollUp,
            Some(steps) if steps > 0.0 => MouseButton::ScrollDown,
            Some(_) | None => return false,
        };
        let target = self
            .windows
            .target_at(pointer.current_location(), self.theme.metrics());
        let command = self
            .config
            .bindings
            .button(
                contexts_of(target.as_ref()),
                self.held_modifiers(),
                button,
                false,
            )
            .and_then(|bindings| bindings.press.cloned());
        let Some(command) = command else {
            return false;
        };
        let window = target.as_ref().map(WindowTarget::window);
        self.execute(&command, &mut ExecutionContext::for_window(window));
        true
    }
}

/// Returns the mouse binding contexts a press on `target` looks in, most
/// specific first.
const fn contexts_of(target: Option<&WindowTarget>) -> &'static [MouseContext] {
    match target {
        Some(WindowTarget::Frame {
            part: FramePart::Titlebar(_),
            ..
        }) => &[MouseContext::Titlebar, MouseContext::Window],
        Some(WindowTarget::Frame {
            part: FramePart::Border(_),
            ..
        }) => &[MouseContext::Border, MouseContext::Window],
        Some(WindowTarget::Surface { .. }) => &[MouseContext::Window],
        None => &[MouseContext::Desktop],
    }
}

/// The input state that outlives a single event.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    /// Keys whose press was a binding: their releases are vaht's too.
    suppressed_keys: HashSet<Keycode>,
    /// The repeating key binding being held, if any.
    key_repeat: Option<KeyRepeat>,
    /// The last button press, to recognize a double click.
    last_click: Option<Click>,
}

impl InputState {
    /// The longest time between the presses of a double click.
    const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(250);

    /// Records a press of `button` on `window` (or the desktop); returns
    /// whether it completes a double click. A third quick press starts a
    /// new one.
    fn is_double_click(&mut self, button: u32, window: Option<WindowId>) -> bool {
        let now = Instant::now();
        let is_double = self.last_click.as_ref().is_some_and(|click| {
            click.button == button
                && click.window == window
                && now.duration_since(click.at) <= Self::DOUBLE_CLICK_INTERVAL
        });
        self.last_click = (!is_double).then_some(Click {
            button,
            window,
            at: now,
        });
        is_double
    }
}

/// A repeating key binding being held.
#[derive(Debug)]
struct KeyRepeat {
    /// The key held.
    keycode: Keycode,
    /// Runs the binding again until dropped.
    _timer: RepeatingTimer<Compositor>,
}

/// A button press, remembered to recognize a double click.
#[derive(Debug, Clone, Copy)]
struct Click {
    /// The button.
    button: u32,
    /// The window pressed, or none for the desktop.
    window: Option<WindowId>,
    /// When.
    at: Instant,
}
