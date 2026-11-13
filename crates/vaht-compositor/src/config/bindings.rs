//! `bindings.toml`: what keys and mouse buttons do (02 §6).
//!
//! ```toml
//! [keys]
//! "Super+T" = "exec kitty"
//!
//! [mouse.titlebar]
//! "Drag+Left" = "start-moving"
//! ```
//!
//! A key chord is modifiers joined by `+`, then an xkb key name. A mouse
//! trigger is modifiers, optionally `Double` or `Drag`, then a button. The
//! file replaces the built-in bindings as a whole, as Fluxbox's `keys` file
//! does, so what it says is everything that is bound.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use serde::de::{self, Deserializer};
use smithay::input::keyboard::ModifiersState;
use smithay::input::keyboard::{Keysym, xkb};

use super::command::Command;
use super::unique_map;
use crate::scene::{PointerTarget, WindowTarget};
use crate::taskbar::TaskbarHit;
use crate::window_manager::{FramePart, GripSide};

/// Every binding.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bindings {
    /// Key chords and what they do.
    #[serde(default, deserialize_with = "unique_map::deserialize")]
    keys: HashMap<KeyChord, Command>,
    /// Mouse triggers and what they do, by where the pointer is.
    #[serde(default)]
    mouse: HashMap<MouseContext, MouseBindings>,
}

/// What a mouse button press is bound to in one context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ButtonBindings<'bindings> {
    /// Run when the button is pressed (or double-clicked).
    pub(crate) press: Option<&'bindings Command>,
    /// Run when the pointer moves while the button is held.
    pub(crate) drag: Option<&'bindings Command>,
}

impl Bindings {
    /// Returns what `chord` is bound to.
    pub(crate) fn key(&self, chord: KeyChord) -> Option<&Command> {
        self.keys.get(&chord)
    }

    /// Returns what turning the wheel `steps` (in 120ths of a notch,
    /// negative up) over `target` with `modifiers` held is bound to.
    pub(crate) fn scroll(
        &self,
        target: Option<&PointerTarget>,
        modifiers: Modifiers,
        steps: f64,
    ) -> Option<&Command> {
        let button = MouseButton::for_scroll(steps)?;
        self.button(MouseContext::search_order(target), modifiers, button, false)?
            .press
    }

    /// Returns what pressing `button` with `modifiers` is bound to in the
    /// first of `contexts` that binds it at all; `is_double` says the press
    /// completes a double click, which a `Double` binding takes over from a
    /// plain one.
    pub(crate) fn button(
        &self,
        contexts: &[MouseContext],
        modifiers: Modifiers,
        button: MouseButton,
        is_double: bool,
    ) -> Option<ButtonBindings<'_>> {
        let trigger = |gesture| MouseTrigger {
            modifiers,
            gesture,
            button,
        };
        contexts.iter().find_map(|context| {
            let bindings = &self.mouse.get(context)?.0;
            let single = bindings.get(&trigger(Gesture::Press));
            let press = if is_double {
                bindings.get(&trigger(Gesture::Double)).or(single)
            } else {
                single
            };
            let drag = bindings.get(&trigger(Gesture::Drag));
            (press.is_some() || drag.is_some()).then_some(ButtonBindings { press, drag })
        })
    }
}

/// A set of modifier keys, one bit each; a binding needs exactly its set
/// held, all others up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct Modifiers(u8);

impl Modifiers {
    /// The modifiers by name, in the order vaht writes them. Fluxbox's names
    /// are accepted too.
    const NAMES: [(&str, &str, Self); 4] = [
        ("Super", "Mod4", Self(1 << 3)),
        ("Ctrl", "Control", Self(1 << 1)),
        ("Alt", "Mod1", Self(1)),
        ("Shift", "Shift", Self(1 << 2)),
    ];

    /// Returns the modifiers held in `state`.
    pub(crate) fn held(state: &ModifiersState) -> Self {
        let [
            (_, _, super_key),
            (_, _, control),
            (_, _, alt),
            (_, _, shift),
        ] = Self::NAMES;
        let bit = |is_held: bool, modifier: Self| if is_held { modifier.0 } else { 0 };
        Self(
            bit(state.logo, super_key)
                | bit(state.ctrl, control)
                | bit(state.alt, alt)
                | bit(state.shift, shift),
        )
    }

    /// Returns the modifier named `name`, in any letter case.
    fn named(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(written, alias, _)| {
                name.eq_ignore_ascii_case(written) || name.eq_ignore_ascii_case(alias)
            })
            .map(|&(.., modifier)| modifier)
    }

    /// Splits a binding's text at `+` into modifiers and the remaining words,
    /// the last of which names the key or button.
    fn split_from(text: &str) -> Result<(Self, Vec<&str>), String> {
        if text.trim() != text || text.contains(char::is_whitespace) {
            return Err(format!(
                "`{text}`: key chains (several chords) are not available in vaht yet, and names \
                 have no spaces"
            ));
        }
        let mut modifiers = Self::default();
        let mut rest = Vec::new();
        let words: Vec<&str> = text.split('+').collect();
        let Some((last, before)) = words.split_last() else {
            return Err("a binding needs a key or button".to_owned());
        };
        for word in before {
            if word.is_empty() {
                return Err(format!("`{text}` has an empty name between `+`s"));
            }
            if let Some(modifier) = Self::named(word) {
                if !modifiers.insert(modifier) {
                    return Err(format!("`{text}` names a modifier twice"));
                }
            } else {
                rest.push(*word);
            }
        }
        rest.push(last);
        Ok((modifiers, rest))
    }

    /// Adds `other`, unless it is already there; returns whether it was new.
    const fn insert(&mut self, other: Self) -> bool {
        let is_new = self.0 & other.0 == 0;
        self.0 |= other.0;
        is_new
    }
}

/// Writes the modifiers as a prefix: `Super+Alt+`.
impl fmt::Display for Modifiers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, _, modifier) in Self::NAMES {
            if self.0 & modifier.0 != 0 {
                write!(formatter, "{name}+")?;
            }
        }
        Ok(())
    }
}

/// A key with modifiers, such as `Super+T`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct KeyChord {
    /// The modifiers held.
    modifiers: Modifiers,
    /// The key, as the keysym it gives without modifiers.
    key: Keysym,
}

impl KeyChord {
    /// The chord of `key` (as its keysym without modifiers) pressed with
    /// `modifiers` held.
    pub(crate) const fn new(modifiers: Modifiers, key: Keysym) -> Self {
        Self { modifiers, key }
    }

    /// Parses `Super+T`.
    fn parse(text: &str) -> Result<Self, String> {
        let (modifiers, words) = Modifiers::split_from(text)?;
        let [key] = words.as_slice() else {
            return Err(format!(
                "`{text}`: modifiers are Super, Ctrl, Alt and Shift; `{}` is not one",
                words.first().copied().unwrap_or_default()
            ));
        };
        // xkb panics on names with NUL in them; no key has one.
        let keysym = if key.is_empty() || key.contains('\0') {
            Keysym::NoSymbol
        } else {
            // Case-insensitive, preferring the lower case: that is the keysym
            // the key gives without Shift, which is what a press is matched by.
            xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE)
        };
        if keysym == Keysym::NoSymbol {
            return Err(format!("`{key}` is not an xkb key name"));
        }
        Ok(Self::new(modifiers, keysym))
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}{}",
            self.modifiers,
            xkb::keysym_get_name(self.key)
        )
    }
}

impl<'de> serde::Deserialize<'de> for KeyChord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

/// Where the pointer is when a mouse binding applies (Fluxbox's `OnWindow`,
/// `OnTitlebar`…). A press on a frame part looks in that part's context
/// first, then in `window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MouseContext {
    /// The desktop: no window.
    Desktop,
    /// The taskbar.
    Taskbar,
    /// A window's entry in the taskbar's window list.
    WindowListEntry,
    /// A window's titlebar, apart from its buttons.
    Titlebar,
    /// Anywhere on a window, frame included.
    Window,
    /// A window's border.
    Border,
    /// The left grip of a window's handle.
    GripLeft,
    /// The right grip of a window's handle.
    GripRight,
    /// A window's handle.
    Handle,
}

impl MouseContext {
    /// Returns the contexts a press on `target` (none: the desktop) looks
    /// in, most specific first: a frame part's own, then `window`; a window
    /// list entry's, then `taskbar`. A layer
    /// surface below the windows (a wallpaper) counts as the desktop; one
    /// above them (a panel) has no bindings: its client handles the press.
    pub(crate) const fn search_order(target: Option<&PointerTarget>) -> &'static [Self] {
        match target {
            Some(PointerTarget::Window(WindowTarget::Frame {
                part: FramePart::Titlebar(_),
                ..
            })) => &[Self::Titlebar, Self::Window],
            Some(PointerTarget::Window(WindowTarget::Frame {
                part: FramePart::Border(_),
                ..
            })) => &[Self::Border, Self::Window],
            Some(PointerTarget::Window(WindowTarget::Frame {
                part: FramePart::Handle,
                ..
            })) => &[Self::Handle, Self::Border, Self::Window],
            Some(PointerTarget::Window(WindowTarget::Frame {
                part: FramePart::Grip(GripSide::Left),
                ..
            })) => &[Self::GripLeft, Self::Window],
            Some(PointerTarget::Window(WindowTarget::Frame {
                part: FramePart::Grip(GripSide::Right),
                ..
            })) => &[Self::GripRight, Self::Window],
            Some(PointerTarget::Window(WindowTarget::Surface { .. })) => &[Self::Window],
            Some(PointerTarget::Taskbar(Some(TaskbarHit::Entry(_)))) => {
                &[Self::WindowListEntry, Self::Taskbar]
            }
            Some(PointerTarget::Taskbar(_)) => &[Self::Taskbar],
            Some(PointerTarget::Layer {
                is_below_windows: true,
                ..
            })
            | None => &[Self::Desktop],
            Some(PointerTarget::Layer {
                is_below_windows: false,
                ..
            }) => &[],
        }
    }
}

/// A mouse button, or a scroll wheel direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MouseButton {
    /// The left button.
    Left,
    /// The middle button (the wheel, pressed).
    Middle,
    /// The right button.
    Right,
    /// The back side button.
    Back,
    /// The forward side button.
    Forward,
    /// The wheel, turned up.
    ScrollUp,
    /// The wheel, turned down.
    ScrollDown,
}

impl MouseButton {
    /// The buttons by name, with their Linux input codes (none for the
    /// wheel).
    const NAMES: [(&str, Self, Option<u32>); 7] = [
        ("Left", Self::Left, Some(0x110)),
        ("Right", Self::Right, Some(0x111)),
        ("Middle", Self::Middle, Some(0x112)),
        ("Back", Self::Back, Some(0x113)),
        ("Forward", Self::Forward, Some(0x114)),
        ("ScrollUp", Self::ScrollUp, None),
        ("ScrollDown", Self::ScrollDown, None),
    ];

    /// Returns the button with Linux input `code`, if bindings can name it.
    pub(crate) fn from_code(code: u32) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(.., button_code)| *button_code == Some(code))
            .map(|&(_, button, _)| button)
    }

    /// Returns the wheel direction of a turn of `steps`: negative is up.
    /// None for no turn.
    fn for_scroll(steps: f64) -> Option<Self> {
        if steps < 0.0 {
            Some(Self::ScrollUp)
        } else if steps > 0.0 {
            Some(Self::ScrollDown)
        } else {
            None
        }
    }

    /// Returns the button named `name`, in any letter case.
    fn named(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(written, ..)| name.eq_ignore_ascii_case(written))
            .map(|&(_, button, _)| button)
    }

    /// Returns whether this is a wheel direction rather than a button.
    const fn is_scroll(self) -> bool {
        matches!(self, Self::ScrollUp | Self::ScrollDown)
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = Self::NAMES
            .iter()
            .find(|(_, button, _)| button == self)
            .map_or("?", |(name, ..)| *name);
        formatter.write_str(name)
    }
}

/// How a button is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Gesture {
    /// Pressed.
    Press,
    /// Pressed twice in quick succession; the second press.
    Double,
    /// Pressed, then moved while held.
    Drag,
}

/// A mouse binding's trigger, such as `Alt+Drag+Left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MouseTrigger {
    /// The modifiers held.
    modifiers: Modifiers,
    /// How the button is used.
    gesture: Gesture,
    /// The button.
    button: MouseButton,
}

impl MouseTrigger {
    /// Parses `Alt+Drag+Left`.
    fn parse(text: &str) -> Result<Self, String> {
        let (modifiers, words) = Modifiers::split_from(text)?;
        let (button_name, gestures) = words
            .split_last()
            .ok_or_else(|| "a mouse binding needs a button".to_owned())?;
        let button = MouseButton::named(button_name).ok_or_else(|| {
            format!(
                "`{button_name}` is not a button: Left, Middle, Right, Back, Forward, ScrollUp \
                 or ScrollDown"
            )
        })?;
        let gesture = match gestures {
            [] => Gesture::Press,
            [word] if word.eq_ignore_ascii_case("Double") => Gesture::Double,
            [word] if word.eq_ignore_ascii_case("Drag") => Gesture::Drag,
            [word] => {
                return Err(format!(
                    "`{text}`: `{word}` is neither a modifier nor Double or Drag"
                ));
            }
            [..] => return Err(format!("`{text}` names more than one of Double and Drag")),
        };
        if button.is_scroll() && gesture != Gesture::Press {
            return Err(format!(
                "`{text}`: the wheel cannot be double-clicked or dragged"
            ));
        }
        Ok(Self {
            modifiers,
            gesture,
            button,
        })
    }
}

impl fmt::Display for MouseTrigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let gesture = match self.gesture {
            Gesture::Press => "",
            Gesture::Double => "Double+",
            Gesture::Drag => "Drag+",
        };
        write!(formatter, "{}{gesture}{}", self.modifiers, self.button)
    }
}

impl<'de> serde::Deserialize<'de> for MouseTrigger {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

/// The mouse bindings of one context.
#[derive(Debug, Clone, Default)]
struct MouseBindings(HashMap<MouseTrigger, Command>);

impl<'de> serde::Deserialize<'de> for MouseBindings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bindings: HashMap<MouseTrigger, Command> = unique_map::deserialize(deserializer)?;
        if let Some((trigger, _)) = bindings.iter().find(|(_, command)| command.repeats()) {
            return Err(de::Error::custom(format!(
                "`{trigger}`: only key bindings repeat"
            )));
        }
        Ok(Self(bindings))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::config::command::{Action, WindowManagerAction};

    /// Parses a bindings file.
    fn bindings(text: &str) -> Result<Bindings, String> {
        toml::from_str(text).map_err(|error| error.message().to_owned())
    }

    /// The modifiers with the keys `names` held: "alt", "super", "shift".
    fn held(names: &[&str]) -> Modifiers {
        Modifiers::held(&ModifiersState {
            alt: names.contains(&"alt"),
            logo: names.contains(&"super"),
            shift: names.contains(&"shift"),
            ..ModifiersState::default()
        })
    }

    #[rstest]
    #[case::up_on_the_desktop(-120.0, None, Some("workspace previous"))]
    #[case::down_on_the_desktop(120.0, None, Some("workspace next"))]
    #[case::no_turn(0.0, None, None)]
    #[case::unbound_on_a_window(
        -120.0,
        Some(PointerTarget::Window(WindowTarget::Frame {
            window: crate::window_manager::WindowId::new(1),
            part: FramePart::Titlebar(vaht_ui::Point::new(1, 1)),
        })),
        None
    )]
    fn the_wheel_runs_what_it_is_bound_to_where_it_turns(
        #[case] steps: f64,
        #[case] target: Option<PointerTarget>,
        #[case] expected: Option<&str>,
    ) {
        let bindings = bindings(
            "[mouse.desktop]\n\"ScrollUp\" = \"workspace previous\"\n\
             \"ScrollDown\" = \"workspace next\"\n",
        )
        .expect("valid");
        let expected = expected.map(|text| Command::Run(Action::parse(text).expect("valid")));
        assert_eq!(
            bindings.scroll(target.as_ref(), Modifiers::default(), steps),
            expected.as_ref()
        );
    }

    #[test]
    fn chords_are_matched_by_modifiers_and_the_unshifted_key() {
        let parsed =
            bindings("[keys]\n\"Mod4+R\" = \"exec fuzzel\"\n\"Alt+Shift+Tab\" = \"raise\"")
                .expect("valid bindings");
        let super_r = KeyChord::new(held(&["super"]), Keysym::r);
        assert_eq!(
            parsed.key(super_r),
            Some(&Command::Run(Action::Exec("fuzzel".to_owned())))
        );
        assert_eq!(super_r.to_string(), "Super+r");
        let alt_shift_tab = KeyChord::new(held(&["alt", "shift"]), Keysym::Tab);
        assert_eq!(
            parsed.key(alt_shift_tab),
            Some(&Command::Run(Action::WindowManager(
                WindowManagerAction::Raise
            )))
        );
        assert_eq!(
            parsed.key(KeyChord::new(held(&["alt"]), Keysym::Tab)),
            None,
            "Alt+Tab is not Alt+Shift+Tab"
        );
    }

    #[test]
    fn a_frame_context_comes_before_window_and_double_falls_back_to_press() {
        let parsed = bindings(
            "[mouse.titlebar]\n\"Left\" = \"raise\"\n\"Drag+Left\" = \"start-moving\"\n\
             [mouse.window]\n\"Left\" = \"lower\"\n\"Alt+Left\" = \"start-moving\"",
        )
        .expect("valid bindings");
        let contexts = [MouseContext::Titlebar, MouseContext::Window];
        let none = Modifiers::default();
        let titlebar = parsed
            .button(&contexts, none, MouseButton::Left, true)
            .expect("the titlebar binds Left");
        assert_eq!(
            titlebar.press,
            Some(&Command::Run(Action::WindowManager(
                WindowManagerAction::Raise
            )))
        );
        assert_eq!(titlebar.drag, Some(&Command::Run(Action::StartMoving)));
        let alt = parsed
            .button(&contexts, held(&["alt"]), MouseButton::Left, false)
            .expect("the window binds Alt+Left");
        assert_eq!(alt.press, Some(&Command::Run(Action::StartMoving)));
        assert_eq!(
            parsed.button(&contexts, none, MouseButton::Right, false),
            None
        );
    }

    #[rstest]
    #[case::unknown_key("[keys]\n\"Super+Banana\" = \"close\"", "not an xkb key name")]
    #[case::unknown_modifier("[keys]\n\"Hyper+T\" = \"close\"", "is not one")]
    #[case::same_chord_twice(
        "[keys]\n\"Super+T\" = \"close\"\n\"Mod4+t\" = \"kill\"",
        "bound twice"
    )]
    #[case::key_chain("[keys]\n\"Super+A Super+B\" = \"close\"", "key chains")]
    #[case::modifier_twice("[keys]\n\"Alt+Alt+T\" = \"close\"", "twice")]
    #[case::unknown_gesture("[mouse.titlebar]\n\"Triple+Left\" = \"close\"", "neither")]
    #[case::dragged_wheel("[mouse.desktop]\n\"Drag+ScrollUp\" = \"close\"", "wheel")]
    #[case::repeating_button(
        "[mouse.desktop]\n\"Left\" = { repeat = \"close\" }",
        "only key bindings"
    )]
    #[case::unknown_context("[mouse.dock]\n\"Left\" = \"close\"", "unknown variant")]
    #[case::unknown_table("[keyz]", "unknown field")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = bindings(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }

    #[rstest]
    #[case::plain("Left")]
    #[case::modified_double("Super+Alt+Double+Right")]
    #[case::modified_drag("Ctrl+Drag+Middle")]
    #[case::wheel("ScrollDown")]
    fn triggers_write_what_they_read(#[case] text: &str) {
        let trigger = MouseTrigger::parse(text).expect(text);
        assert_eq!(trigger.to_string(), text);
    }
}
