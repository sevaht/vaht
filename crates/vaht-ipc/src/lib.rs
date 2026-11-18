//! The vaht IPC protocol: the messages exchanged over vaht's Unix socket,
//! and a small blocking client.
//!
//! One JSON message per line, in both directions: a client writes a
//! [`Request`] and reads one [`Reply`] for it, in order; a client that
//! subscribed also reads [`Event`]s. The types here are
//! the single source of the protocol: vaht answers with them, `vaht msg` and
//! the dialogs ask with them (02 §12).
//!
//! vaht listens at [`socket_path`] and tells the programs it starts where,
//! in [`SOCKET_VARIABLE`].

mod client;
mod config_path;
mod message;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub use client::{Client, ClientError};
pub use config_path::{ConfigPath, ConfigPathError};
pub use message::{
    ConfigEdit, ConfigFile, ConfigFileInfo, ConfigState, Event, EventKind, ModeInfo, OutputInfo,
    Reply, Request, WindowInfo, WorkspaceInfo,
};

/// The environment variable that names vaht's IPC socket, set for every
/// program vaht starts.
pub const SOCKET_VARIABLE: &str = "VAHT_SOCKET";

/// The longest message either side accepts, in bytes, its newline
/// included: far more than any request needs, so a peer that sends more is
/// broken or hostile, and is disconnected rather than buffered.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Returns the protocol's JSON Schema (draft 2020-12): every message is a
/// [`Request`], a [`Reply`] or an [`Event`], each defined in `$defs`, which
/// is how other languages can check or generate their own types.
///
/// # Panics
///
/// Never: the schema is plain data that always encodes.
#[must_use]
pub fn schema() -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let messages = [
        generator.subschema_for::<Request>(),
        generator.subschema_for::<Reply>(),
        generator.subschema_for::<Event>(),
    ];
    let definitions = generator.take_definitions(true);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "vaht IPC message",
        "description": "One JSON object per line over vaht's IPC socket: a request a client sends, \
                        or a reply or event vaht sends.",
        "oneOf": messages,
        "$defs": definitions,
    })
}

/// Returns where the vaht serving Wayland display `display` listens:
/// beside the display's own socket in `runtime_directory`.
#[must_use]
pub fn socket_path(runtime_directory: &Path, display: &OsStr) -> PathBuf {
    let mut name = display.to_os_string();
    name.push(".vaht.sock");
    runtime_directory.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_defines_every_message_and_kind() {
        let schema = schema();
        let definitions = schema["$defs"].as_object().expect("definitions");
        for name in ["Request", "Reply", "Event", "WindowInfo", "EventKind"] {
            assert!(definitions.contains_key(name), "{name}: {schema}");
        }
        let request = definitions["Request"].to_string();
        for kind in [
            "version",
            "run",
            "windows",
            "workspaces",
            "outputs",
            "subscribe",
        ] {
            assert!(
                request.contains(&format!("\"{kind}\"")),
                "{kind}: {request}"
            );
        }
    }

    #[test]
    fn the_socket_sits_beside_the_displays() {
        assert_eq!(
            socket_path(Path::new("/run/user/1000"), OsStr::new("wayland-1")),
            Path::new("/run/user/1000/wayland-1.vaht.sock")
        );
    }
}
