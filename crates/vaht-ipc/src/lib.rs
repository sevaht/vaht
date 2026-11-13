//! The vaht IPC protocol: the messages exchanged over vaht's Unix socket,
//! and a small blocking client.
//!
//! One JSON message per line, in both directions: a client writes a
//! [`Request`] and reads one [`Reply`] for it, in order. The types here are
//! the single source of the protocol: vaht answers with them, `vaht msg` and
//! the dialogs ask with them (02 §12).
//!
//! vaht listens at [`socket_path`] and tells the programs it starts where,
//! in [`SOCKET_VARIABLE`].

mod client;
mod message;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub use client::{Client, ClientError};
pub use message::{Reply, Request, WindowInfo, WorkspaceInfo};

/// The environment variable that names vaht's IPC socket, set for every
/// program vaht starts.
pub const SOCKET_VARIABLE: &str = "VAHT_SOCKET";

/// The longest message either side accepts, in bytes, its newline
/// included: far more than any request needs, so a peer that sends more is
/// broken or hostile, and is disconnected rather than buffered.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

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
    fn the_socket_sits_beside_the_displays() {
        assert_eq!(
            socket_path(Path::new("/run/user/1000"), OsStr::new("wayland-1")),
            Path::new("/run/user/1000/wayland-1.vaht.sock")
        );
    }
}
