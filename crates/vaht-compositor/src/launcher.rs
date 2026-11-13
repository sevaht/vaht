//! Starting programs, and reaping them when they exit (03 §2).
//!
//! [`Launcher`] is the only place vaht starts processes. Each command line
//! runs as `sh -c '<command line>'`, and vaht's dialogs as `vaht dialog
//! <name>`, each:
//! - in a process group of its own, so a signal meant for vaht's group
//!   does not reach it;
//! - with no signals blocked, and SIGPIPE back at its default action. vaht
//!   blocks the signals it reads as events and ignores SIGPIPE (Rust does),
//!   and a new program inherits both; the standard library's `Command`
//!   resets neither, which a test found: programs started from vaht could
//!   not be stopped with SIGTERM. `posix_spawn` sets both safely.
//! - with no input, and its output discarded.
//!
//! Every program is kept, and counted, until it has exited and been reaped:
//! vaht is told of exits by SIGCHLD (see `signals`), so no zombie outlives
//! its program.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// Starts programs and reaps them.
#[derive(Debug)]
pub(crate) struct Launcher {
    /// The programs started that have not been reaped yet.
    running: Vec<(Pid, Tracked)>,
    /// Variables set for every program, on top of vaht's own environment:
    /// `WAYLAND_DISPLAY`, `VAHT_SOCKET`, and whatever `set-environment`
    /// set.
    environment: BTreeMap<OsString, OsString>,
    /// vaht's own program, which shows its dialogs.
    executable: PathBuf,
    /// Counts running programs.
    ledger: ResourceLedger,
}

impl Launcher {
    /// The shell that runs command lines.
    const SHELL: &str = "/bin/sh";

    /// Creates a launcher whose programs connect to `socket_name`, and
    /// whose dialogs are shown by `executable`.
    pub(crate) fn new(socket_name: OsString, executable: PathBuf, ledger: &ResourceLedger) -> Self {
        Self {
            running: Vec::new(),
            environment: BTreeMap::from([(OsString::from("WAYLAND_DISPLAY"), socket_name)]),
            executable,
            ledger: ledger.clone(),
        }
    }

    /// Starts `command_line` with the shell.
    pub(crate) fn run(&mut self, command_line: &str) {
        let arguments = ["sh", "-c", command_line].map(OsStr::new);
        self.start(Path::new(Self::SHELL), &arguments, command_line);
    }

    /// Shows vaht's dialog `name` (`vaht dialog <name>`), styled by the
    /// configuration in `config_directory`.
    pub(crate) fn show_dialog(&mut self, name: &str, config_directory: &Path) {
        let executable = self.executable.clone();
        let arguments = [
            OsStr::new("vaht"),
            OsStr::new("--config-directory"),
            config_directory.as_os_str(),
            OsStr::new("dialog"),
            OsStr::new(name),
        ];
        self.start(&executable, &arguments, &format!("the {name} dialog"));
    }

    /// Sets `name` to `value` for every program started from now on.
    pub(crate) fn set_environment(&mut self, name: String, value: String) {
        self.environment.insert(name.into(), value.into());
    }

    /// Reaps every program that has exited.
    pub(crate) fn reap(&mut self) {
        self.running
            .retain(|(pid, _)| match waitpid(*pid, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) => true,
                Ok(status) => {
                    tracing::debug!(%pid, ?status, "program exited");
                    // Stopped and continued programs are still running.
                    !matches!(status, WaitStatus::Exited(..) | WaitStatus::Signaled(..))
                }
                Err(error) => {
                    tracing::warn!(%error, %pid, "could not check on a program; no longer tracking it");
                    false
                }
            });
    }

    /// Starts `program` with `arguments` (its own name first), keeping it
    /// until it is reaped; `what` names it in the log.
    fn start(&mut self, program: &Path, arguments: &[&OsStr], what: &str) {
        match self.spawn(program, arguments) {
            Ok(pid) => {
                tracing::debug!(%pid, what, "program started");
                self.running
                    .push((pid, self.ledger.track(ResourceKind::Program)));
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                what,
                "could not start a program"
            ),
        }
    }

    /// Starts `program` with `arguments` as described in the module
    /// documentation.
    fn spawn(&self, program: &Path, arguments: &[&OsStr]) -> Result<Pid, SpawnError> {
        let arguments = arguments
            .iter()
            .map(|argument| CString::new(argument.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(SpawnError::Nul)?;
        let mut attributes = PosixSpawnAttr::init().map_err(SpawnError::Prepare)?;
        attributes
            .set_flags(
                PosixSpawnFlags::POSIX_SPAWN_SETPGROUP
                    | PosixSpawnFlags::POSIX_SPAWN_SETSIGMASK
                    | PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF,
            )
            .map_err(SpawnError::Prepare)?;
        attributes
            .set_pgroup(Pid::from_raw(0))
            .map_err(SpawnError::Prepare)?;
        attributes
            .set_sigmask(&SigSet::empty())
            .map_err(SpawnError::Prepare)?;
        let mut defaults = SigSet::empty();
        defaults.add(Signal::SIGPIPE);
        attributes
            .set_sigdefault(&defaults)
            .map_err(SpawnError::Prepare)?;
        // Input and output go to /dev/null: dup2 onto 0, 1 and 2 in the
        // child. vaht's own descriptor is closed on exec, and here on return.
        let null = File::options()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map_err(SpawnError::OpenNull)?;
        let mut file_actions = PosixSpawnFileActions::init().map_err(SpawnError::Prepare)?;
        for standard in 0..=2 {
            file_actions
                .add_dup2(null.as_raw_fd(), standard)
                .map_err(SpawnError::Prepare)?;
        }
        posix_spawn(
            program,
            &file_actions,
            &attributes,
            &arguments,
            &self.environment_block(),
        )
        .map_err(SpawnError::Spawn)
    }

    /// Returns vaht's environment with this launcher's variables set, as
    /// `NAME=value` strings.
    fn environment_block(&self) -> Vec<CString> {
        let mut variables: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        variables.extend(self.environment.clone());
        variables
            .iter()
            .filter_map(|(name, value)| {
                let mut entry = name.as_bytes().to_vec();
                entry.push(b'=');
                entry.extend_from_slice(OsStr::as_bytes(value));
                // The environment cannot hold NUL; set-environment could.
                CString::new(entry).ok()
            })
            .collect()
    }
}

/// Why a program could not be started.
#[derive(Debug, thiserror::Error)]
enum SpawnError {
    /// An argument contains a NUL character.
    #[error("an argument contains a NUL character")]
    Nul(#[source] std::ffi::NulError),
    /// The spawn attributes could not be set up.
    #[error("could not prepare to start the program")]
    Prepare(#[source] nix::Error),
    /// `/dev/null` could not be opened for the program's input and output.
    #[error("could not open /dev/null")]
    OpenNull(#[source] std::io::Error),
    /// The program could not be started.
    #[error("could not start the program")]
    Spawn(#[source] nix::Error),
}
