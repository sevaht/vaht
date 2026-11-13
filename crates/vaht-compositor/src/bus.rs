//! Which D-Bus message bus vaht talks to, and connecting to it.
//!
//! vaht reads system services from the system bus (timedated, for time
//! zone changes) and will host the tray on the session bus. A [`Bus`] names
//! one of them, or a bus at an address, which is how tests give vaht a
//! private bus of their own. Connections are zbus's; their futures run on
//! the event loop ([`crate::event_source::StreamSource`]).

use zbus::Connection;

/// A D-Bus message bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bus {
    /// The system-wide bus, where system services such as timedated are.
    System,
    /// The bus at a D-Bus address, such as `unix:path=/run/test-bus`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only tests run private buses yet")
    )]
    At(String),
}

impl Bus {
    /// Connects to the bus.
    ///
    /// # Errors
    ///
    /// Returns zbus's error if the bus cannot be reached or refuses vaht.
    pub(crate) async fn connect(&self) -> zbus::Result<Connection> {
        match self {
            Self::System => Connection::system().await,
            Self::At(address) => {
                zbus::connection::Builder::address(address.as_str())
                    .build()
                    .await
            }
        }
    }
}

/// A message bus of a test's own: a `dbus-daemon` that lives as long as
/// this value.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PrivateBus {
    /// The daemon.
    daemon: std::process::Child,
    /// Where its socket is.
    _directory: tempfile::TempDir,
    /// Its address.
    address: String,
}

#[cfg(test)]
impl PrivateBus {
    /// Starts a bus.
    ///
    /// # Panics
    ///
    /// Panics if `dbus-daemon` cannot be run.
    pub(crate) fn start() -> Self {
        use std::io::BufRead;

        let directory = tempfile::tempdir().expect("a temporary directory");
        let mut daemon = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:dir={}", directory.path().display()))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("dbus-daemon is installed");
        let mut address = String::new();
        std::io::BufReader::new(daemon.stdout.take().expect("piped"))
            .read_line(&mut address)
            .expect("dbus-daemon prints its address");
        Self {
            daemon,
            _directory: directory,
            address: address.trim().to_owned(),
        }
    }

    /// Returns the bus, to connect to.
    pub(crate) fn bus(&self) -> Bus {
        Bus::At(self.address.clone())
    }

    /// Stops the bus, as a crashed or restarted daemon would: every
    /// connection to it is lost.
    pub(crate) fn stop(&mut self) {
        // A daemon already gone needs nothing more.
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

#[cfg(test)]
impl Drop for PrivateBus {
    fn drop(&mut self) {
        self.stop();
    }
}
