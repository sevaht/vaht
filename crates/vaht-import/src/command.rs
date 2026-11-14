//! Translating a Fluxbox command (`:MacroCmd {Raise} {Focus}`) into vaht's
//! command language (02 §4).
//!
//! A translated command is a TOML value, written as vaht writes commands:
//! the string form where it can be, a table where a matcher or a repeat
//! needs one. Parts vaht deliberately lacks (tabs) are left out of a
//! sequence with a note; a part vaht does not have *yet* leaves the whole
//! command untranslated, since half a sequence could do the wrong thing.

use toml_edit::{Array, InlineTable, Value};

use crate::pattern::{self, ClientPattern};
use crate::report::Reason;

/// A command translated, and what it lost on the way.
#[derive(Debug, Clone)]
pub(crate) struct Translated {
    /// The vaht command.
    pub(crate) value: Value,
    /// Parts left out, each with why.
    pub(crate) losses: Vec<(Reason, String)>,
}

/// Why a command was not translated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Untranslated {
    /// Why.
    pub(crate) reason: Reason,
    /// What could not be translated.
    pub(crate) detail: String,
}

impl Untranslated {
    /// A command vaht deliberately lacks.
    fn dropped(detail: impl Into<String>) -> Self {
        Self {
            reason: Reason::Dropped,
            detail: detail.into(),
        }
    }

    /// A command vaht lacks for now.
    fn not_yet(detail: impl Into<String>) -> Self {
        Self {
            reason: Reason::NotYetInVaht,
            detail: detail.into(),
        }
    }
}

/// One Fluxbox command, as written after a binding's colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FluxboxCommand<'text>(pub(crate) &'text str);

impl FluxboxCommand<'_> {
    /// Fluxbox commands vaht deliberately has no counterpart for.
    const DROPPED: [&str; 10] = [
        "activatetab",
        "starttabbing",
        "nexttab",
        "prevtab",
        "tab",
        "movetableft",
        "movetabright",
        "detachclient",
        "setdecor",
        "setstyle",
    ];

    /// Returns the vaht command, or why there is none.
    pub(crate) fn to_vaht(self) -> Result<Translated, Untranslated> {
        let text = self.0.trim();
        let (name, arguments) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(name, arguments)| (name, arguments.trim()));
        let lowercase = name.to_ascii_lowercase();
        match lowercase.as_str() {
            "macrocmd" => Self::sequence(arguments),
            "repeat" => Self::repeat(arguments),
            "exec" | "execute" | "execcommand" if Self::runs_a_run_window(arguments) => {
                Ok(Self::run_dialog(arguments))
            }
            "nextwindow" => Ok(Self::cycle("next-window", arguments)),
            "prevwindow" => Ok(Self::cycle("previous-window", arguments)),
            _ => Self::action(&lowercase, arguments).map(|action| Translated {
                value: Value::from(action),
                losses: Vec::new(),
            }),
        }
    }

    /// Translates a plain action named `name` (lowercase) with `arguments`
    /// into vaht's string form.
    fn action(name: &str, arguments: &str) -> Result<String, Untranslated> {
        let plain = |vaht: &str| Ok(vaht.to_owned());
        let with = |vaht: &str| Ok(format!("{vaht} {arguments}").trim_end().to_owned());
        match name {
            "exec" | "execute" | "execcommand" => with("exec"),
            "close" => plain("close"),
            "kill" | "killwindow" => plain("kill"),
            "raise" => plain("raise"),
            "lower" => plain("lower"),
            "focus" => plain("focus"),
            "startmoving" => plain("start-moving"),
            "startresizing" => Self::resize_corner(arguments),
            "minimize" | "minimizewindow" | "iconify" => plain("minimize"),
            "maximize" | "maximizewindow" => plain("maximize"),
            "maximizehorizontal" => plain("maximize horizontal"),
            "maximizevertical" => plain("maximize vertical"),
            "fullscreen" => plain("fullscreen"),
            "shade" | "shadewindow" => plain("shade"),
            "stick" | "stickwindow" => plain("stick"),
            "snapstep" => with("snap-step"),
            "showdesktop" => plain("show-desktop"),
            "sendtonexthead" => plain("send-to-output next"),
            "sendtoprevhead" => plain("send-to-output previous"),
            "sethead" if arguments.parse::<std::num::NonZeroUsize>().is_ok() => {
                with("send-to-output")
            }
            "toggledecor" => plain("toggle-frame"),
            "setworkspacename" if !arguments.is_empty() => with("rename-workspace"),
            "toggletoolbarhidden" => plain("toggle-taskbar"),
            "focusleft" => plain("focus-direction left"),
            "focusright" => plain("focus-direction right"),
            "focusup" => plain("focus-direction up"),
            "focusdown" => plain("focus-direction down"),
            "markwindow" if !arguments.is_empty() => with("mark-window"),
            "gotomarkedwindow" if !arguments.is_empty() => with("go-to-marked-window"),
            "nextworkspace" => plain("workspace next"),
            "prevworkspace" => plain("workspace previous"),
            "workspace" => with("workspace"),
            "sendtoworkspace" => with("send-to-workspace"),
            "taketoworkspace" => with("take-to-workspace"),
            "sendtonextworkspace" => plain("send-to-workspace next"),
            "sendtoprevworkspace" => plain("send-to-workspace previous"),
            "taketonextworkspace" => plain("take-to-workspace next"),
            "taketoprevworkspace" => plain("take-to-workspace previous"),
            "addworkspace" => plain("add-workspace"),
            "removelastworkspace" => plain("remove-last-workspace"),
            "move" => with("move"),
            "moveto" => with("move-to"),
            "resize" => with("resize"),
            "resizeto" => with("resize-to"),
            "reconfigure" | "reconfig" => plain("reload"),
            // fluxland's Restart reloads; with a command it would start
            // another window manager instead.
            "restart" if arguments.is_empty() => plain("reload"),
            "exit" | "quit" => plain("exit"),
            "setalpha" => with("set-alpha"),
            "settitle" => with("set-title"),
            "settitledialog" => plain("set-title-dialog"),
            "setlayer" => Self::layer(arguments)
                .map(|layer| format!("set-layer {layer}"))
                .ok_or_else(|| Untranslated::not_yet(format!("SetLayer {arguments}"))),
            "rundialog" => plain("run-dialog"),
            "commanddialog" => plain("command-dialog"),
            "rootmenu" => plain("root-menu"),
            "windowmenu" => plain("window-menu"),
            "workspacemenu" => plain("workspace-menu"),
            "hidemenus" => plain("hide-menus"),
            // A pattern choosing which windows ClientMenu lists is lost:
            // vaht's window-list menu lists every window.
            "clientmenu" if arguments.is_empty() => plain("window-list-menu"),
            "setenv" | "export" => Ok(format!(
                "set-environment {}",
                arguments.replacen('=', " ", 1)
            )),
            dropped if Self::DROPPED.contains(&dropped) => Err(Untranslated::dropped(format!(
                "{name} (tabs and decoration commands are gone)"
            ))),
            other => Err(Untranslated::not_yet(
                format!("the command {other} {arguments}")
                    .trim_end()
                    .to_owned(),
            )),
        }
    }

    /// Fluxbox's layers, by name (lowercase) and number, and vaht's names.
    const LAYERS: [(&str, &str, &str); 6] = [
        ("abovedock", "2", "above-dock"),
        ("dock", "4", "dock"),
        ("top", "6", "top"),
        ("normal", "8", "normal"),
        ("bottom", "10", "bottom"),
        ("desktop", "12", "desktop"),
    ];

    /// Returns vaht's name for the Fluxbox layer `text` names, by name or
    /// number.
    pub(crate) fn layer(text: &str) -> Option<&'static str> {
        let text = text.trim().to_ascii_lowercase();
        Self::LAYERS
            .iter()
            .find(|(name, number, _)| *name == text || *number == text)
            .map(|(_, _, vaht)| *vaht)
    }

    /// The programs that show a run window: Fluxbox's and fluxland's.
    const RUN_PROGRAMS: [&str; 2] = ["fbrun", "fluxland-run"];

    /// Returns whether the command line `arguments` (of an `Exec`) starts
    /// a run window, which vaht's run dialog replaces.
    fn runs_a_run_window(arguments: &str) -> bool {
        arguments
            .split_whitespace()
            .next()
            .is_some_and(|program| Self::RUN_PROGRAMS.contains(&program))
    }

    /// Translates `Exec fbrun [options]` to the run dialog; its options
    /// (font, colors, size, place) are the theme's and the rules' to say.
    fn run_dialog(arguments: &str) -> Translated {
        let options = arguments
            .split_once(char::is_whitespace)
            .map_or("", |(_, options)| options.trim());
        let losses = if options.is_empty() {
            Vec::new()
        } else {
            vec![(
                Reason::Dropped,
                format!(
                    "the run window's options ({options}): the theme's dialog.field styles \
                     the run dialog, and rules place it"
                ),
            )]
        };
        Translated {
            value: Value::from("run-dialog"),
            losses,
        }
    }

    /// Translates `StartResizing`'s corner.
    fn resize_corner(arguments: &str) -> Result<String, Untranslated> {
        let corner = match arguments.to_ascii_lowercase().as_str() {
            "" | "nearestcorner" => "nearest-corner",
            "topleft" => "top-left",
            "top" => "top",
            "topright" => "top-right",
            "left" => "left",
            "right" => "right",
            "bottomleft" => "bottom-left",
            "bottom" => "bottom",
            "bottomright" => "bottom-right",
            other => {
                return Err(Untranslated::not_yet(format!("StartResizing {other}")));
            }
        };
        Ok(format!("start-resizing {corner}"))
    }

    /// Translates `MacroCmd {A} {B}`: a sequence, without the parts vaht
    /// deliberately lacks.
    fn sequence(arguments: &str) -> Result<Translated, Untranslated> {
        let mut commands = Array::new();
        let mut losses = Vec::new();
        for part in Self::braced(arguments) {
            match FluxboxCommand(part).to_vaht() {
                Ok(translated) => {
                    commands.push(translated.value);
                    losses.extend(translated.losses);
                }
                Err(Untranslated {
                    reason: Reason::Dropped,
                    detail,
                }) => losses.push((Reason::Dropped, detail)),
                Err(untranslated) => return Err(untranslated),
            }
        }
        let value = match commands.len() {
            0 => return Err(Untranslated::dropped("a sequence of nothing vaht has")),
            1 => commands.get(0).cloned().expect("one command"),
            _ => Value::Array(commands),
        };
        Ok(Translated { value, losses })
    }

    /// Translates `Repeat {A}`.
    fn repeat(arguments: &str) -> Result<Translated, Untranslated> {
        let inner = Self::braced(arguments)
            .into_iter()
            .next()
            .ok_or_else(|| Untranslated::not_yet(format!("Repeat {arguments}")))?;
        let translated = FluxboxCommand(inner).to_vaht()?;
        let mut table = InlineTable::new();
        table.insert("repeat", translated.value);
        Ok(Translated {
            value: Value::InlineTable(table),
            losses: translated.losses,
        })
    }

    /// Translates `NextWindow {options} (pattern)` into `action` with a
    /// window matcher.
    fn cycle(action: &str, arguments: &str) -> Translated {
        let mut losses = Vec::new();
        let mut is_static = false;
        for option in Self::braced(arguments)
            .iter()
            .flat_map(|options| options.split_whitespace())
        {
            match option.to_ascii_lowercase().as_str() {
                "groups" => losses.push((
                    Reason::Dropped,
                    "the groups option (tabs are gone)".to_owned(),
                )),
                "static" => is_static = true,
                other => losses.push((Reason::NotYetInVaht, format!("the {other} option"))),
            }
        }
        let (windows, lost) = ClientPattern::new(arguments).matcher();
        losses.extend(lost);
        if windows.is_empty() && !is_static {
            return Translated {
                value: Value::from(action),
                losses,
            };
        }
        let mut table = InlineTable::new();
        table.insert("action", Value::from(action));
        if !windows.is_empty() {
            table.insert("windows", Value::InlineTable(windows));
        }
        // Fluxbox's static order is the order windows opened in.
        if is_static {
            table.insert("order", Value::from("opening"));
        }
        Translated {
            value: Value::InlineTable(table),
            losses,
        }
    }

    /// Returns the contents of the top-level `{...}` groups in `text`.
    fn braced(text: &str) -> Vec<&str> {
        pattern::groups(text, '{', '}')
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn directional_focus_and_marks_translate() {
        let vaht = |fluxbox| {
            FluxboxCommand(fluxbox)
                .to_vaht()
                .map(|translated| translated.value.as_str().unwrap_or_default().to_owned())
        };
        assert_eq!(vaht("FocusLeft").as_deref(), Ok("focus-direction left"));
        assert_eq!(vaht("FocusDown").as_deref(), Ok("focus-direction down"));
        assert_eq!(vaht("MarkWindow a").as_deref(), Ok("mark-window a"));
        assert_eq!(
            vaht("GotoMarkedWindow a").as_deref(),
            Ok("go-to-marked-window a")
        );
        assert_eq!(vaht("SendToNextHead").as_deref(), Ok("send-to-output next"));
        assert_eq!(vaht("SetHead 2").as_deref(), Ok("send-to-output 2"));
        assert_eq!(vaht("ToggleDecor").as_deref(), Ok("toggle-frame"));
        assert_eq!(
            vaht("SetWorkspaceName Mail").as_deref(),
            Ok("rename-workspace Mail")
        );
        assert_eq!(vaht("ToggleToolbarHidden").as_deref(), Ok("toggle-taskbar"));
    }

    /// Translates `command` and returns the vaht value as TOML text.
    fn translate(command: &str) -> String {
        FluxboxCommand(command).to_vaht().map_or_else(
            |untranslated| format!("{untranslated:?}"),
            |translated| translated.value.to_string().trim().to_owned(),
        )
    }

    #[rstest]
    #[case::exec("Exec kitty", r#""exec kitty""#)]
    #[case::macro_without_tabs("MacroCmd {Raise} {Focus} {ActivateTab}", r#"["raise", "focus"]"#)]
    #[case::macro_of_one("MacroCmd {Raise} {ActivateTab}", r#""raise""#)]
    #[case::resize_corner("StartResizing bottomleft", r#""start-resizing bottom-left""#)]
    #[case::repeat(
        "Repeat {Exec amixer sset Master 2%+}",
        r#"{ repeat = "exec amixer sset Master 2%+" }"#
    )]
    #[case::cycle_on_workspace(
        "NextWindow {groups} (workspace=[current])",
        r#"{ action = "next-window", windows = { workspace = "current" } }"#
    )]
    #[case::take_previous("TakeToPrevWorkspace", r#""take-to-workspace previous""#)]
    #[case::fluxland_snap("SnapStep left", r#""snap-step left""#)]
    #[case::run_dialog("RunDialog", r#""run-dialog""#)]
    #[case::set_alpha("SetAlpha 200 +10", r#""set-alpha 200 +10""#)]
    #[case::set_layer("SetLayer AboveDock", r#""set-layer above-dock""#)]
    #[case::set_title_dialog("SetTitleDialog", r#""set-title-dialog""#)]
    #[case::restart("Restart", r#""reload""#)]
    #[case::set_layer_by_number("SetLayer 10", r#""set-layer bottom""#)]
    #[case::command_dialog("CommandDialog", r#""command-dialog""#)]
    #[case::fbrun("Exec fbrun", r#""run-dialog""#)]
    #[case::fluxland_run("ExecCommand fluxland-run -f 'Sans 10'", r#""run-dialog""#)]
    #[case::not_a_run_window("Exec fbrunner", r#""exec fbrunner""#)]
    fn commands_translate(#[case] fluxbox: &str, #[case] vaht: &str) {
        assert_eq!(translate(fluxbox), vaht);
    }

    #[rstest]
    #[case::tabs("StartTabbing", Reason::Dropped)]
    #[case::menu_from_a_file("CustomMenu ~/.fluxbox/custom", Reason::NotYetInVaht)]
    #[case::macro_with_a_missing_part("MacroCmd {Raise} {CustomMenu x}", Reason::NotYetInVaht)]
    fn commands_not_translated_say_why(#[case] fluxbox: &str, #[case] reason: Reason) {
        let untranslated = FluxboxCommand(fluxbox).to_vaht().expect_err(fluxbox);
        assert_eq!(untranslated.reason, reason);
    }

    #[test]
    fn a_run_windows_options_are_noted() {
        let translated = FluxboxCommand("Exec fbrun -font sans")
            .to_vaht()
            .expect("translated");
        assert_eq!(translated.losses.len(), 1);
        assert!(translated.losses[0].1.contains("-font sans"));
    }

    #[test]
    fn my_taskbar_scrolling_cycles_in_opening_order() {
        let translated = FluxboxCommand("PrevWindow {static groups} (iconhidden=no)")
            .to_vaht()
            .expect("translated");
        assert_eq!(
            translated.value.to_string().trim(),
            r#"{ action = "previous-window", order = "opening" }"#
        );
        let reasons: Vec<_> = translated
            .losses
            .iter()
            .map(|(reason, _)| *reason)
            .collect();
        assert_eq!(
            reasons,
            [Reason::Dropped],
            "only the groups, as tabs are gone"
        );
        let unknown = FluxboxCommand("NextWindow {fancy} (iconhidden=yes)")
            .to_vaht()
            .expect("translated without them");
        assert_eq!(unknown.losses.len(), 2, "{:?}", unknown.losses);
    }
}
