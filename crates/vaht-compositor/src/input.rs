//! The input state that outlives a single event, and its decisions.
//!
//! [`Input`] decides whose a key event is (a bound press, and its release,
//! are vaht's and never reach the client) and what a button press does
//! ([`PressOutcome`]), and keeps the timers that repeat a held key binding
//! and raise a window the pointer rests on. Its decisions are values: the
//! compositor carries them out through the seat (01 §7).

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use smithay::backend::input::{
    Axis, AxisSource, Event as _, InputBackend, KeyState, PointerAxisEvent as _,
};
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::AxisFrame;
use smithay::reexports::calloop::LoopHandle;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::Compositor;
use crate::config::{Bindings, Command, Config, KeyChord, Modifiers, MouseButton, MouseContext};
use crate::event_source::{RepeatingTimer, Timer};
use crate::execution::ExecutionContext;
use crate::resources::ResourceLedger;
use crate::scene::{PointerTarget, WindowTarget};
use crate::window_manager::{FramePart, ResizeEdges, WindowId};

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

/// A button press, with what decides what it does.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ButtonPress<'target> {
    /// The button's Linux input code.
    pub(crate) button: u32,
    /// What is under the pointer; none for the bare desktop.
    pub(crate) target: Option<&'target PointerTarget>,
    /// The modifiers held.
    pub(crate) modifiers: Modifiers,
    /// Whether the pointer is on a titlebar button.
    pub(crate) is_on_titlebar_button: bool,
    /// When it happened.
    pub(crate) time: Instant,
}

/// What a button press does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PressOutcome {
    /// A layer surface above the windows (a panel, a notification) was
    /// pressed: the press is its client's alone, and the surface may take
    /// the keyboard.
    ClickLayer(WlSurface),
    /// A window, or the desktop, was pressed.
    Click {
        /// The window pressed, which gets focus; none for the desktop,
        /// which takes it from every window.
        window: Option<WindowId>,
        /// Whether the window is raised first.
        raises: bool,
        /// What happens then.
        then: AfterClick,
    },
}

/// What a press on a window or the desktop does after focusing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AfterClick {
    /// The titlebar button of this window is held down until released.
    HoldTitlebarButton(WindowId),
    /// The press is bound: `press` runs now, and the press is held from the
    /// client, `drag` running if the pointer moves first.
    RunBinding {
        /// Run on the press.
        press: Option<Box<Command>>,
        /// Run once the press becomes a drag.
        drag: Option<Box<Command>>,
    },
    /// An unbound press on this window's border resizes it by `edges`.
    ResizeBorder(WindowId, ResizeEdges),
    /// Nothing of vaht's: the client under the pointer gets the press.
    PassToClient,
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
    /// How long a held key waits before repeating; repeating key bindings
    /// use it too.
    pub(crate) const KEY_REPEAT_DELAY: Duration = Duration::from_millis(200);

    /// How often a held key repeats, per second; repeating key bindings
    /// use it too, unless they set their own rate.
    pub(crate) const KEY_REPEAT_RATE: NonZeroU32 =
        NonZeroU32::new(25).expect("the key repeat rate is not zero");

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

    /// Decides what `press` does under `config`: on a panel, it is the
    /// panel's; otherwise the window pressed is focused (and raised, if the
    /// settings say so), then a titlebar button pressed without modifiers
    /// is held, a bound press runs its binding, and an unbound press on a
    /// border resizes. A double click is recognized here, so the press is
    /// recorded.
    pub(crate) fn press(&mut self, press: &ButtonPress<'_>, config: &Config) -> PressOutcome {
        let target = press.target;
        if let Some(PointerTarget::Layer {
            surface,
            is_below_windows: false,
            ..
        }) = target
        {
            return PressOutcome::ClickLayer(surface.clone());
        }
        let window = target.and_then(PointerTarget::window);
        let then = match target {
            Some(PointerTarget::Window(WindowTarget::Frame {
                window,
                part: FramePart::Titlebar(_),
            })) if press.is_on_titlebar_button && press.modifiers == Modifiers::default() => {
                AfterClick::HoldTitlebarButton(*window)
            }
            _ => self.bound_or_border(press, config),
        };
        PressOutcome::Click {
            window,
            raises: window.is_some() && config.settings.focus.raise_on_click,
            then,
        }
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
        let interval = Duration::from_secs(1) / rate.unwrap_or(Self::KEY_REPEAT_RATE).get();
        let started = RepeatingTimer::start(
            handle,
            ledger,
            Self::KEY_REPEAT_DELAY,
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

    /// Returns what `press`, past the titlebar buttons, does: its binding,
    /// or a border resize, or nothing.
    fn bound_or_border(&mut self, press: &ButtonPress<'_>, config: &Config) -> AfterClick {
        let window = press.target.and_then(PointerTarget::window);
        let interval = config.settings.windows.double_click_interval();
        let is_double = self.is_double_click(press.button, window, press.time, interval);
        let bound = MouseButton::from_code(press.button).and_then(|button| {
            config.bindings.button(
                MouseContext::search_order(press.target),
                press.modifiers,
                button,
                is_double,
            )
        });
        if let Some(bindings) = bound {
            return AfterClick::RunBinding {
                press: bindings.press.cloned().map(Box::new),
                drag: bindings.drag.cloned().map(Box::new),
            };
        }
        match press.target {
            Some(PointerTarget::Window(WindowTarget::Frame {
                window,
                part: FramePart::Border(edges),
            })) => AfterClick::ResizeBorder(*window, *edges),
            Some(
                PointerTarget::Window(WindowTarget::Frame { .. } | WindowTarget::Surface { .. })
                | PointerTarget::Layer { .. },
            )
            | None => AfterClick::PassToClient,
        }
    }

    /// Records a press of `button` on `window` (or the desktop) at `now`;
    /// returns whether it completes a double click, within `interval` of
    /// the press before. A third quick press starts a new one.
    fn is_double_click(
        &mut self,
        button: u32,
        window: Option<WindowId>,
        now: Instant,
        interval: Duration,
    ) -> bool {
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::window_manager::{HorizontalEdge, VerticalEdge};

    /// The left button's Linux input code.
    const LEFT: u32 = 0x110;

    /// The Alt key, as bindings name it.
    fn alt() -> Modifiers {
        Modifiers::held(&smithay::input::keyboard::ModifiersState {
            alt: true,
            ..Default::default()
        })
    }

    /// The window every press lands on.
    fn window() -> WindowId {
        WindowId::new(7)
    }

    /// The edges of the right border.
    fn right_edge() -> ResizeEdges {
        ResizeEdges {
            horizontal: HorizontalEdge::Right,
            vertical: VerticalEdge::None,
        }
    }

    /// What is under the pointer.
    #[derive(Debug, Clone, Copy)]
    enum Under {
        /// The bare desktop.
        Desktop,
        /// The window's titlebar, off its buttons.
        Titlebar,
        /// One of the window's titlebar buttons.
        TitlebarButton,
        /// The window's right border.
        Border,
    }

    impl Under {
        /// Returns the pointer target, and whether it is a titlebar button.
        fn target(self) -> (Option<PointerTarget>, bool) {
            let frame = |part| {
                Some(PointerTarget::Window(WindowTarget::Frame {
                    window: window(),
                    part,
                }))
            };
            let titlebar = FramePart::Titlebar(vaht_ui::Point::new(5, 5));
            match self {
                Self::Desktop => (None, false),
                Self::Titlebar => (frame(titlebar), false),
                Self::TitlebarButton => (frame(titlebar), true),
                Self::Border => (frame(FramePart::Border(right_edge())), false),
            }
        }
    }

    /// The configuration with `bindings` and `settings`, as files hold them.
    fn config(bindings: &str, settings: &str) -> Config {
        let mut config = Config::built_in();
        config.bindings = toml::from_str::<Bindings>(bindings).expect("valid bindings");
        config.settings = toml::from_str(settings).expect("valid settings");
        config
    }

    /// Bindings on the titlebar and on windows with Alt.
    const BINDINGS: &str = "[mouse.titlebar]\n\"Double+Left\" = \"maximize\"\n\
                            [mouse.window]\n\"Alt+Left\" = \"start-moving\"\n";

    /// Presses the left button once on `under` with `modifiers` held.
    fn press(
        input: &mut Input,
        config: &Config,
        under: Under,
        modifiers: Modifiers,
        time: Instant,
    ) -> PressOutcome {
        let (target, is_on_titlebar_button) = under.target();
        input.press(
            &ButtonPress {
                button: LEFT,
                target: target.as_ref(),
                modifiers,
                is_on_titlebar_button,
                time,
            },
            config,
        )
    }

    /// Returns what the press does after focusing, and whether it raises.
    fn then(outcome: PressOutcome) -> (AfterClick, bool) {
        match outcome {
            PressOutcome::Click { then, raises, .. } => (then, raises),
            PressOutcome::ClickLayer(_) => panic!("no layer surface was pressed"),
        }
    }

    /// The outcome of a press bound to run the action `text`, with no drag.
    fn runs(text: &str) -> AfterClick {
        let action = crate::config::Action::parse(text).expect("a valid action");
        AfterClick::RunBinding {
            press: Some(Box::new(Command::Run(action))),
            drag: None,
        }
    }

    #[rstest]
    #[case::titlebar_button(Under::TitlebarButton, false, AfterClick::HoldTitlebarButton(window()))]
    #[case::titlebar_button_with_alt_runs_the_window_binding(
        Under::TitlebarButton,
        true,
        runs("start-moving")
    )]
    #[case::unbound_titlebar(Under::Titlebar, false, AfterClick::PassToClient)]
    #[case::unbound_border(Under::Border, false, AfterClick::ResizeBorder(window(), right_edge()))]
    #[case::bound_border(Under::Border, true, runs("start-moving"))]
    #[case::unbound_desktop(Under::Desktop, false, AfterClick::PassToClient)]
    fn a_press_does_what_is_under_it_and_its_bindings_say(
        #[case] under: Under,
        #[case] holds_alt: bool,
        #[case] expected: AfterClick,
    ) {
        let config = config(BINDINGS, "");
        let modifiers = if holds_alt {
            alt()
        } else {
            Modifiers::default()
        };
        let outcome = press(
            &mut Input::default(),
            &config,
            under,
            modifiers,
            Instant::now(),
        );
        assert_eq!(then(outcome).0, expected);
    }

    #[rstest]
    #[case::raised_by_default("", Under::Titlebar, true)]
    #[case::not_when_the_settings_say_so(
        "[focus]\nraise_on_click = false\n",
        Under::Titlebar,
        false
    )]
    #[case::nothing_to_raise_on_the_desktop("", Under::Desktop, false)]
    fn a_pressed_window_is_raised_if_the_settings_say_so(
        #[case] settings: &str,
        #[case] under: Under,
        #[case] raises: bool,
    ) {
        let config = config(BINDINGS, settings);
        let outcome = press(
            &mut Input::default(),
            &config,
            under,
            Modifiers::default(),
            Instant::now(),
        );
        assert_eq!(then(outcome).1, raises);
    }

    #[test]
    fn a_second_quick_press_is_a_double_click_and_a_third_starts_over() {
        let config = config(BINDINGS, "");
        let mut input = Input::default();
        let start = Instant::now();
        let quickly = config.settings.windows.double_click_interval() / 2;
        let outcomes: Vec<AfterClick> = (0..3)
            .map(|count| {
                let time = start + quickly * count;
                then(press(
                    &mut input,
                    &config,
                    Under::Titlebar,
                    Modifiers::default(),
                    time,
                ))
                .0
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                AfterClick::PassToClient,
                runs("maximize"),
                AfterClick::PassToClient,
            ]
        );
    }

    #[test]
    fn presses_too_far_apart_are_not_a_double_click() {
        let config = config(BINDINGS, "");
        let mut input = Input::default();
        let start = Instant::now();
        let slowly = config.settings.windows.double_click_interval() * 2;
        press(
            &mut input,
            &config,
            Under::Titlebar,
            Modifiers::default(),
            start,
        );
        let second = press(
            &mut input,
            &config,
            Under::Titlebar,
            Modifiers::default(),
            start + slowly,
        );
        assert_eq!(then(second).0, AfterClick::PassToClient);
    }

    #[test]
    fn a_titlebar_button_press_does_not_count_toward_a_double_click() {
        let config = config(BINDINGS, "");
        let mut input = Input::default();
        let now = Instant::now();
        press(
            &mut input,
            &config,
            Under::TitlebarButton,
            Modifiers::default(),
            now,
        );
        let next = press(
            &mut input,
            &config,
            Under::Titlebar,
            Modifiers::default(),
            now,
        );
        assert_eq!(then(next).0, AfterClick::PassToClient);
    }
}
