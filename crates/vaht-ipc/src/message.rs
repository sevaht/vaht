//! The requests a client sends and the replies vaht sends back.
//!
//! Each is a JSON object naming its kind in one field (`"request"` or
//! `"reply"`), with the kind's own fields beside it:
//!
//! ```json
//! {"request":"run","command":"workspace 2"}
//! {"reply":"done"}
//! ```

use serde::{Deserialize, Serialize};

/// What a client asks of vaht.
///
/// Requests without fields are written `Version {}` rather than as unit
/// variants, so a field they do not take is refused like any other: serde
/// ignores unknown fields beside a unit variant's tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// What vaht answers a request with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The request could not be carried out, or was not understood.
    Error {
        /// Why, for a person to read.
        message: String,
    },
}

/// One window, as IPC describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// One workspace, as IPC describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInfo {
    /// Its number, from 1.
    pub number: usize,
    /// Its name.
    pub name: String,
    /// Whether it is the one shown.
    pub is_current: bool,
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
