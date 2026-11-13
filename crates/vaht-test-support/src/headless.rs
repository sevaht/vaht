//! A `vaht --backend headless` process, and what it reports as it stops.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// How long to wait for vaht to reach a state before failing the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// How often to look at the log while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The resource kinds that exist only on behalf of clients; all must be gone
/// once the clients are.
const CLIENT_RESOURCES: [&str; 7] = [
    "client",
    "window",
    "toplevel",
    "frame",
    "frame-texture",
    "capture-session",
    "layer-surface",
];

/// A `vaht --backend headless` process with private directories: its
/// socket, its configuration and its log never meet the developer's own.
///
/// Dropping it kills vaht, so a failing test never leaves one running.
#[derive(Debug)]
pub struct HeadlessVaht {
    /// The process.
    process: Child,
    /// Its `XDG_RUNTIME_DIR`, where its socket is; removed with the value.
    runtime: tempfile::TempDir,
    /// Its configuration directory; removed with the value.
    config: tempfile::TempDir,
    /// Where it logs; removed with the value.
    logs: tempfile::TempDir,
    /// The socket clients connect to.
    socket: PathBuf,
}

impl HeadlessVaht {
    /// What vaht logs in tests: debug detail, plus the per-frame line that
    /// [`Self::wait_for_disconnections`] waits for.
    const LOG_FILTER: &str = "debug,vaht_compositor::compositor=trace";

    /// Starts `executable` (the `vaht` binary) headless with the built-in
    /// configuration, and waits until it accepts clients.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot start or does not accept clients in time.
    #[must_use]
    pub fn start(executable: &Path) -> Self {
        Self::start_with_config(executable, &[])
    }

    /// Starts `executable` headless with configuration `files`, each a file
    /// name and its contents, and waits until it accepts clients.
    ///
    /// # Panics
    ///
    /// Panics if the files cannot be written, or vaht cannot start or does
    /// not accept clients in time.
    #[must_use]
    pub fn start_with_config(executable: &Path, files: &[(&str, &str)]) -> Self {
        let config = tempfile::tempdir().expect("a temporary configuration directory");
        for (name, contents) in files {
            let path = config.path().join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("the file's directory is made");
            }
            std::fs::write(path, contents).expect("the file is written");
        }
        // Socket paths are limited to about 108 bytes, so the runtime
        // directory must be short: the system's temporary directory is.
        let runtime = tempfile::Builder::new()
            .prefix("vaht-")
            .tempdir()
            .expect("a temporary runtime directory");
        let logs = tempfile::tempdir().expect("a temporary log directory");
        let process = Command::new(executable)
            .args([
                "--backend",
                "headless",
                "--journald",
                "off",
                "--log",
                Self::LOG_FILTER,
            ])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("VAHT_LOG_DIRECTORY", logs.path())
            .env("VAHT_CONFIG_DIRECTORY", config.path())
            .env_remove("VAHT_LOG")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the vaht executable starts");
        let mut vaht = Self {
            process,
            socket: PathBuf::new(),
            runtime,
            config,
            logs,
        };
        let line = vaht.wait_for_log(|log| {
            log.lines()
                .find(|line| line.contains("accepting clients"))
                .map(ToOwned::to_owned)
        });
        let name = line
            .split_whitespace()
            .find_map(|field| field.strip_prefix("socket="))
            .expect("the accepting line names the socket");
        vaht.socket = vaht.runtime.path().join(name);
        vaht
    }

    /// Returns the socket clients connect to.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Returns vaht's configuration directory, where a test can change the
    /// files while vaht runs.
    #[must_use]
    pub fn config_directory(&self) -> &Path {
        self.config.path()
    }

    /// Waits until vaht's log contains `text` at least `count` times.
    ///
    /// # Panics
    ///
    /// Panics if that does not happen in time.
    pub fn wait_for_logged(&self, text: &str, count: usize) {
        self.wait_for_log(|log| (log.matches(text).count() >= count).then_some(()));
    }

    /// Returns how many times vaht's log so far contains `text`.
    #[must_use]
    pub fn logged_count(&self, text: &str) -> usize {
        self.read_log().matches(text).count()
    }

    /// Waits until vaht has logged `count` client disconnections and drawn
    /// a frame after the last one.
    ///
    /// A gone client's last buffers are let go when the frame after its
    /// windows close is drawn, a frame later at most; stopping vaht before
    /// then would report them as leaked.
    ///
    /// # Panics
    ///
    /// Panics if that does not happen in time.
    pub fn wait_for_disconnections(&self, count: usize) {
        self.wait_for_log(|log| {
            let (index, _) = log
                .match_indices("client disconnected")
                .nth(count.checked_sub(1)?)?;
            log[index..].contains("frame drawn").then_some(())
        });
    }

    /// Stops vaht with SIGTERM and returns its log.
    ///
    /// # Panics
    ///
    /// Panics if vaht cannot be signalled or does not stop cleanly.
    #[must_use]
    pub fn stop(mut self) -> ExitReport {
        let pid = i32::try_from(self.process.id()).expect("process ids fit in an i32");
        kill(Pid::from_raw(pid), Signal::SIGTERM).expect("vaht can be signalled");
        let status = self.process.wait().expect("vaht can be waited for");
        assert!(status.success(), "vaht did not stop cleanly: {status}");
        ExitReport {
            log: self.read_log(),
        }
    }

    /// Waits for vaht to exit by itself (the `exit` action) and returns its
    /// log.
    ///
    /// # Panics
    ///
    /// Panics if vaht does not exit cleanly in time.
    #[must_use]
    pub fn wait_for_exit(mut self) -> ExitReport {
        let deadline = Instant::now() + PATIENCE;
        let status = loop {
            if let Some(status) = self.process.try_wait().expect("vaht can be waited for") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "vaht did not exit:\n{}",
                self.read_log()
            );
            std::thread::sleep(POLL_INTERVAL);
        };
        assert!(status.success(), "vaht did not exit cleanly: {status}");
        ExitReport {
            log: self.read_log(),
        }
    }

    /// Returns the log so far.
    fn read_log(&self) -> String {
        std::fs::read_to_string(self.logs.path().join("vaht.log")).unwrap_or_default()
    }

    /// Polls the log until `found` finds what it looks for.
    fn wait_for_log<T>(&self, found: impl Fn(&str) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let log = self.read_log();
            if let Some(value) = found(&log) {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "vaht never got there; log:\n{log}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for HeadlessVaht {
    fn drop(&mut self) {
        // Errors mean it has already exited, which is what drop wants.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// What vaht reported as it stopped.
#[derive(Debug)]
pub struct ExitReport {
    /// The whole log.
    log: String,
}

impl ExitReport {
    /// Returns the live count of `kind` in the exit resource report.
    ///
    /// # Panics
    ///
    /// Panics if the report or the kind is missing.
    #[must_use]
    pub fn live_count(&self, kind: &str) -> usize {
        let report = self
            .log
            .split("resources at exit")
            .nth(1)
            .expect("vaht logged its resources at exit");
        report
            .lines()
            .find_map(|line| {
                let rest = line.trim_start().strip_prefix(kind)?.strip_prefix(": ")?;
                rest.split_whitespace().next()?.parse().ok()
            })
            .unwrap_or_else(|| panic!("the exit report has no {kind} count:\n{}", self.log))
    }

    /// Returns how many times the log contains `text`.
    #[must_use]
    pub fn log_count(&self, text: &str) -> usize {
        self.log.matches(text).count()
    }

    /// Returns the open descriptor count vaht logged at `moment` ("at start"
    /// or "at exit").
    ///
    /// # Panics
    ///
    /// Panics if vaht logged none.
    #[must_use]
    pub fn descriptors(&self, moment: &str) -> usize {
        let marker = format!("process usage {moment}");
        self.log
            .lines()
            .find(|line| line.contains(&marker))
            .and_then(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("descriptors="))
            })
            .and_then(|count| count.parse().ok())
            .unwrap_or_else(|| panic!("vaht logged no descriptor count {moment}:\n{}", self.log))
    }

    /// Asserts that nothing vaht acquired for clients outlived them, and
    /// that it holds as many descriptors as when it started.
    ///
    /// # Panics
    ///
    /// Panics, with the log, if anything was left behind.
    pub fn assert_nothing_left_behind(&self) {
        for kind in CLIENT_RESOURCES {
            assert_eq!(
                self.live_count(kind),
                0,
                "{kind} left behind:\n{}",
                self.log
            );
        }
        assert_eq!(
            self.descriptors("at exit"),
            self.descriptors("at start"),
            "descriptors leaked:\n{}",
            self.log
        );
    }
}
