//! The requests a client sends and the replies vaht sends back.
//!
//! Each is a JSON object naming its kind in one field (`"request"` or
//! `"reply"`), with the kind's own fields beside it:
//!
//! ```json
//! {"request":"run","command":"workspace 2"}
//! {"reply":"done"}
//! ```
//!
//! A client that subscribes is then sent an [`Event`] (`"event"`) with the
//! whole of each list it subscribed to, at once and whenever it changes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ConfigPath;

/// What a client asks of vaht.
///
/// Requests without fields are written `Version {}` rather than as unit
/// variants, so a field they do not take is refused like any other: serde
/// ignores unknown fields beside a unit variant's tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "request", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// vaht's version.
    Version {},
    /// Runs a command, written as a binding writes one (`"workspace 2"`,
    /// `"exec kitty"`).
    Run {
        /// The command.
        command: String,
        /// The window it acts on, by id; the focused window when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u64>,
    },
    /// The windows, topmost first.
    Windows {},
    /// The workspaces, in order.
    Workspaces {},
    /// The outputs, on and off, in the order vaht found them.
    Outputs {},
    /// Sends the lists `events` names now, and again whenever they change,
    /// as [`Event`]s on this connection.
    Subscribe {
        /// The lists.
        events: Vec<EventKind>,
    },
    /// Which events vaht records, as tracing filter directives: changed to
    /// `directives` if given, back to the filter vaht started with if
    /// `reset`, else just told. Answered with the filter in effect.
    LogFilter {
        /// The new filter, such as `info,vaht_compositor::input=trace`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        directives: Option<String>,
        /// Whether to go back to the filter vaht started with.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reset: bool,
    },
    /// Which configuration files can be read, and whether each has changes
    /// not written yet, with the styles available.
    ConfigFiles {},
    /// What `file` holds, described as JSON Schema (draft 2020-12): every
    /// key with its type, documentation, default and range, and vaht's own
    /// `x-vaht-*` hints.
    ConfigSchema {
        /// The file.
        file: ConfigFile,
    },
    /// What `file` says now: its effective value (the file with the
    /// changes made in place on top), what vaht defaults to, and which
    /// keys are changed.
    ConfigRead {
        /// The file.
        file: ConfigFile,
    },
    /// Makes `edits` to `file` in place, all of them or none: vaht checks the
    /// whole configuration with them made, runs it at once if it is valid,
    /// and answers `Done`, or an error saying what is wrong and changing
    /// nothing. The changes stay in place, not in the file, until
    /// [`Request::ConfigSave`].
    ConfigEdit {
        /// The file.
        file: ConfigFile,
        /// What to change, in order.
        edits: Vec<ConfigEdit>,
    },
    /// Writes every change made in place into its file, keeping the file's
    /// comments and layout, and answers `Done`.
    ConfigSave {},
    /// Puts `file` back to what it says: the changes made in place are
    /// dropped, and what it says runs.
    ConfigRevert {
        /// The file.
        file: ConfigFile,
    },
    /// Shows `items` as a vaht menu at the pointer, titled `title`, and
    /// answers with the one the user chooses, or none if they close the
    /// menu: a dmenu-style picker (the screen-share portal's chooser). The
    /// reply waits for the user; requests sent meanwhile on the
    /// connection are answered after it, in order.
    Pick {
        /// The menu's title, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The items, top to bottom.
        items: Vec<String>,
    },
}

/// A list a client may subscribe to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    /// The windows, as [`Request::Windows`] answers.
    Windows,
    /// The workspaces, as [`Request::Workspaces`] answers.
    Workspaces,
    /// The outputs, as [`Request::Outputs`] answers.
    Outputs,
    /// The configuration: sent whenever vaht runs a new one (the files
    /// changed, a change made in place, saved or reverted).
    Config,
}

/// A list a client subscribed to, sent at once and whenever it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Event {
    /// The windows, topmost first.
    Windows {
        /// Each window.
        windows: Vec<WindowInfo>,
    },
    /// The workspaces, in order.
    Workspaces {
        /// Each workspace.
        workspaces: Vec<WorkspaceInfo>,
    },
    /// The outputs, in the order vaht found them.
    Outputs {
        /// Each output.
        outputs: Vec<OutputInfo>,
        /// The display profile applied, if one is.
        profile: Option<String>,
    },
    /// The configuration vaht runs changed: read what it says again.
    Config {
        /// Counts the configurations vaht has run, so each is news even if
        /// its files' states are as before.
        generation: u64,
        /// Each file and whether it has changes not written.
        files: Vec<ConfigFileInfo>,
        /// What could not be used of the files and was left out, a sentence
        /// each.
        problems: Vec<String>,
    },
}

impl Event {
    /// Returns the reply asking for this list now would get.
    #[must_use]
    pub fn into_reply(self) -> Reply {
        match self {
            Self::Windows { windows } => Reply::Windows { windows },
            Self::Workspaces { workspaces } => Reply::Workspaces { workspaces },
            Self::Outputs { outputs, profile } => Reply::Outputs { outputs, profile },
            Self::Config {
                files, problems, ..
            } => Reply::ConfigFiles {
                files,
                styles: Vec::new(),
                layouts: Vec::new(),
                style_options: Vec::new(),
                problems,
            },
        }
    }

    /// Returns the list this is.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Windows { .. } => EventKind::Windows,
            Self::Workspaces { .. } => EventKind::Workspaces,
            Self::Outputs { .. } => EventKind::Outputs,
            Self::Config { .. } => EventKind::Config,
        }
    }
}

/// One change to a configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "edit", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConfigEdit {
    /// Sets the setting at `path` to `value`.
    Set {
        /// Where the setting is.
        path: ConfigPath,
        /// Its new value, in the shape [`Request::ConfigRead`] gives it.
        value: serde_json::Value,
    },
    /// Puts the setting at `path` back to its default: it is left out of the
    /// file.
    Unset {
        /// Where the setting is.
        path: ConfigPath,
    },
}

/// One of the files vaht reads its configuration from.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFile {
    /// `settings.toml`: the settings.
    Settings,
    /// `outputs.toml`: monitors and display profiles.
    Outputs,
    /// `bindings.toml`: keys and mouse buttons.
    Bindings,
    /// `rules.toml`: what happens to particular windows.
    Rules,
    /// `menus.toml`: the menus.
    Menus,
}

impl ConfigFile {
    /// Returns the file's name in the configuration directory.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Settings => "settings.toml",
            Self::Outputs => "outputs.toml",
            Self::Bindings => "bindings.toml",
            Self::Rules => "rules.toml",
            Self::Menus => "menus.toml",
        }
    }
}

/// Whether a configuration file has changes not written to it yet.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigState {
    /// Vaht runs what the file says.
    Clean,
    /// Vaht runs changes made in place, which saving writes into the file.
    Changed,
}

/// A configuration file vaht can describe and read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileInfo {
    /// Which file.
    pub file: ConfigFile,
    /// Whether it has changes not written.
    pub state: ConfigState,
}

/// An option the style in use declares (02 §10.5), with the choice in effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleOptionInfo {
    /// Its name, as `style_options` names it.
    pub name: String,
    /// What to call it for people, if the style says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The choice made when none is.
    pub default: String,
    /// The choice in effect.
    pub chosen: String,
    /// The choices that can be made: the style's own and the shared ones,
    /// the default first, the rest sorted.
    pub choices: Vec<String>,
}

/// What vaht answers a request with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reply", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Reply {
    /// vaht's version, for [`Request::Version`].
    Version {
        /// The version, as `vaht --version` prints it.
        version: String,
    },
    /// The request was carried out.
    Done {},
    /// The windows, for [`Request::Windows`].
    Windows {
        /// Each window, topmost first.
        windows: Vec<WindowInfo>,
    },
    /// The workspaces, for [`Request::Workspaces`].
    Workspaces {
        /// Each workspace, in order.
        workspaces: Vec<WorkspaceInfo>,
    },
    /// The outputs, for [`Request::Outputs`].
    Outputs {
        /// Each output, in the order vaht found them.
        outputs: Vec<OutputInfo>,
        /// The display profile applied, if one is.
        profile: Option<String>,
    },
    /// The log filter in effect, for [`Request::LogFilter`].
    LogFilter {
        /// Its directives.
        directives: String,
    },
    /// The files that can be read and the styles, for
    /// [`Request::ConfigFiles`].
    ConfigFiles {
        /// Each file.
        files: Vec<ConfigFileInfo>,
        /// The styles that can be chosen: those in the configuration
        /// directory's `styles/` and the system's, and the built-in one.
        styles: Vec<String>,
        /// The layouts that can be chosen: those in the configuration
        /// directory's `layouts/` and the system's.
        layouts: Vec<String>,
        /// The options the style in use declares, each with the choice in
        /// effect; empty for a style with none.
        #[serde(default)]
        style_options: Vec<StyleOptionInfo>,
        /// What vaht could not use of the configuration files and left out,
        /// a sentence each; empty when all of it is in effect.
        problems: Vec<String>,
    },
    /// A file's schema, for [`Request::ConfigSchema`].
    ConfigSchema {
        /// JSON Schema (draft 2020-12) of the file.
        schema: serde_json::Value,
    },
    /// What a file says now, for [`Request::ConfigRead`].
    ConfigRead {
        /// The effective value, in the file's own shape, every key present.
        value: serde_json::Value,
        /// What vaht defaults to, in the same shape.
        defaults: serde_json::Value,
        /// The keys changed in place and not written, each as a path
        /// (`["focus", "model"]`).
        changed: Vec<Vec<String>>,
        /// The layout `settings.toml` names, if it names one. `defaults` is
        /// vaht's defaults under that layout.
        #[serde(default)]
        layout: Option<String>,
        /// The settings that come from the layout (what it sets that
        /// `settings.toml` does not), each as a path.
        #[serde(default)]
        from_layout: Vec<Vec<String>>,
    },
    /// The item chosen, for [`Request::Pick`]: none if the menu was closed
    /// without a choice.
    Picked {
        /// The item, as sent.
        choice: Option<String>,
    },
    /// The request could not be carried out, or was not understood.
    Error {
        /// Why, for a person to read.
        message: String,
    },
}

/// One window, as IPC describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowInfo {
    /// Its id, which no other window ever has while vaht runs.
    pub id: u64,
    /// Its title.
    pub title: String,
    /// Its application id (Wayland's `app_id`).
    pub app_id: String,
    /// The number of its workspace, from 1; none when it is on every one.
    pub workspace: Option<usize>,
    /// Whether it has keyboard focus.
    pub is_focused: bool,
    /// Whether it is minimized.
    pub is_minimized: bool,
    /// Whether it is rolled up into its titlebar.
    pub is_shaded: bool,
    /// The id of the window it is a dialog of, if any.
    #[serde(default)]
    pub parent: Option<u64>,
}

/// One workspace, as IPC describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInfo {
    /// Its number, from 1.
    pub number: usize,
    /// Its name.
    pub name: String,
    /// Whether it is the one shown.
    pub is_current: bool,
}

/// One output, as IPC describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputInfo {
    /// Its connector's name, such as `HDMI-A-1`.
    pub connector: String,
    /// The display's manufacturer, as its EDID gives it.
    pub make: String,
    /// The display's model.
    pub model: String,
    /// The display's serial number; empty when it gives none.
    pub serial: String,
    /// Whether it is on.
    pub is_enabled: bool,
    /// Whether it is the primary output.
    pub is_primary: bool,
    /// Its mode, when it is on.
    pub mode: Option<ModeInfo>,
    /// Where its top left corner is in the layout, in logical pixels.
    pub position: [i32; 2],
    /// How much larger everything is drawn on it.
    pub scale: f64,
}

/// An output's mode, as IPC describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModeInfo {
    /// Its width, in pixels.
    pub width: i32,
    /// Its height, in pixels.
    pub height: i32,
    /// Its refresh rate, in millihertz.
    pub refresh_millihertz: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_as_the_protocol_writes_them() {
        let request: Request =
            serde_json::from_str(r#"{"request":"run","command":"workspace 2"}"#).expect("valid");
        assert_eq!(
            request,
            Request::Run {
                command: "workspace 2".to_owned(),
                window: None
            }
        );
        let reply = serde_json::to_string(&Reply::Done {}).expect("serializable");
        assert_eq!(reply, r#"{"reply":"done"}"#);
    }

    #[test]
    fn unknown_requests_and_fields_are_refused() {
        assert!(serde_json::from_str::<Request>(r#"{"request":"dance"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"request":"version","extra":1}"#).is_err());
    }
}
