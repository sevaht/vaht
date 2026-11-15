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

/// What a client asks of vaht.
///
/// Requests without fields are written `Version {}` rather than as unit
/// variants, so a field they do not take is refused like any other: serde
/// ignores unknown fields beside a unit variant's tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
}

impl Event {
    /// Returns the reply asking for this list now would get.
    #[must_use]
    pub fn into_reply(self) -> Reply {
        match self {
            Self::Windows { windows } => Reply::Windows { windows },
            Self::Workspaces { workspaces } => Reply::Workspaces { workspaces },
            Self::Outputs { outputs, profile } => Reply::Outputs { outputs, profile },
        }
    }

    /// Returns the list this is.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Windows { .. } => EventKind::Windows,
            Self::Workspaces { .. } => EventKind::Workspaces,
            Self::Outputs { .. } => EventKind::Outputs,
        }
    }
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
