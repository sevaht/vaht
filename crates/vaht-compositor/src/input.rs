//! The input state that outlives a single event, and its decisions.
//!
//! [`Input`] decides whose a key event is (a bound press, and its release,
//! are vaht's and never reach the client), recognizes double clicks, and
//! keeps the timers that repeat a held key binding and raise a window the
//! pointer rests on. The compositor routes events through it and the seat
//! (01 §7).

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use smithay::backend::input::{
    Axis, AxisSource, Event as _, InputBackend, KeyState, PointerAxisEvent as _,
};
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::AxisFrame;
use smithay::reexports::calloop::LoopHandle;

use crate::Compositor;
use crate::config::{Bindings, Command, KeyChord};
use crate::event_source::{RepeatingTimer, Timer};
use crate::execution::ExecutionContext;
use crate::resources::ResourceLedger;
use crate::window_manager::WindowId;

/// Scroll distance per wheel notch, in the 120ths that high-resolution
/// wheels report: a notch is 120, and libinput maps it to 15 units.
const SCROLL_UNITS_PER_V120: f64 = 15.0 / 120.0;

/// Returns the scroll `event` as the frame a client receives.
pub(crate) fn axis_frame<B: InputBackend>(event: &B::PointerAxisEvent) -> AxisFrame {
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
    frame
}

/// The input state that outlives a single event.
#[derive(Debug, Default)]
pub(crate) struct Input {
    /// Keys whose press was a binding: their releases are vaht's too.
    suppressed_keys: HashSet<Keycode>,
    /// The repeating key binding being held, if any.
    key_repeat: Option<KeyRepeat>,
    /// The last button press, to recognize a double click.
    last_click: Option<Click>,
    /// Raises the window the pointer rests on, with auto-raise.
    auto_raise: Option<Timer<Compositor>>,
}

impl Input {
    /// Decides whose a key event is: the client's, or vaht's, with the
    /// command a bound press runs. `chord` is the press
    /// with its modifiers, if the key has a keysym. A bound press's release
    /// is vaht's too, and any press or that key's release ends a repeat.
    pub(crate) fn key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        chord: Option<KeyChord>,
        bindings: &Bindings,
    ) -> FilterResult<Option<Command>> {
        match state {
            KeyState::Pressed => {
                self.key_repeat = None;
                // A fresh press of a key whose release was never seen.
                self.suppressed_keys.remove(&keycode);
                let Some((chord, command)) =
                    chord.and_then(|chord| Some((chord, bindings.key(chord)?)))
                else {
                    return FilterResult::Forward;
                };
                tracing::debug!(%chord, "key binding");
                self.suppressed_keys.insert(keycode);
                FilterResult::Intercept(Some(command.clone()))
            }
            KeyState::Released => {
                if self
                    .key_repeat
                    .as_ref()
                    .is_some_and(|repeat| repeat.keycode == keycode)
                {
                    self.key_repeat = None;
                }
                if self.suppressed_keys.remove(&keycode) {
                    FilterResult::Intercept(None)
                } else {
                    FilterResult::Forward
                }
            }
        }
    }

    /// Records a press of `button` on `window` (or the desktop); returns
    /// whether it completes a double click, within `interval` of the press
    /// before. A third quick press starts a new one.
    pub(crate) fn is_double_click(
        &mut self,
        button: u32,
        window: Option<WindowId>,
        interval: Duration,
    ) -> bool {
        let now = Instant::now();
        let is_double = self.last_click.as_ref().is_some_and(|click| {
            click.button == button
                && click.window == window
                && now.duration_since(click.at) <= interval
        });
        self.last_click = (!is_double).then_some(Click {
            button,
            window,
            at: now,
        });
        is_double
    }

    /// Runs `command`, bound to `keycode`, again and again at `rate` per
    /// second (the keyboard's own when `None`) while the key is held.
    pub(crate) fn repeat_key_binding(
        &mut self,
        keycode: Keycode,
        command: Command,
        rate: Option<NonZeroU32>,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let (handle, ledger) = event_loop;
        let interval = Duration::from_secs(1) / rate.unwrap_or(Compositor::KEY_REPEAT_RATE).get();
        let started = RepeatingTimer::start(
            handle,
            ledger,
            Compositor::KEY_REPEAT_DELAY,
            interval,
            move |compositor: &mut Compositor| {
                let window = compositor.window_manager.focused();
                compositor.execute(&command, &mut ExecutionContext::for_window(window));
            },
        );
        match started {
            Ok(timer) => {
                self.key_repeat = Some(KeyRepeat {
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

    /// Raises `window` once `delay` has passed, if it still has focus then;
    /// replaces any raise pending.
    pub(crate) fn raise_later(
        &mut self,
        window: WindowId,
        delay: Duration,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) {
        let (handle, ledger) = event_loop;
        let raise = Timer::once(handle, ledger, delay, move |compositor: &mut Compositor| {
            if compositor.window_manager.focused() == Some(window) {
                compositor.window_manager.raise(window);
            }
        });
        match raise {
            Ok(timer) => self.auto_raise = Some(timer),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not schedule raising a window"
            ),
        }
    }

    /// Cancels a raise pending.
    pub(crate) fn cancel_raise(&mut self) {
        self.auto_raise = None;
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
