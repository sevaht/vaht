//! Starting programs, and reaping them when they exit (03 §2).
//!
//! [`Launcher`] is the only place vaht starts processes. Each program runs
//! as `sh -c '<command line>'`:
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
    /// `WAYLAND_DISPLAY`, and whatever `set-environment` set.
    environment: BTreeMap<OsString, OsString>,
    /// Counts running programs.
    ledger: ResourceLedger,
}

impl Launcher {
    /// The shell that runs command lines.
    const SHELL: &str = "/bin/sh";

    /// Creates a launcher whose programs connect to `socket_name`.
    pub(crate) fn new(socket_name: OsString, ledger: &ResourceLedger) -> Self {
        Self {
            running: Vec::new(),
            environment: BTreeMap::from([(OsString::from("WAYLAND_DISPLAY"), socket_name)]),
            ledger: ledger.clone(),
        }
    }

    /// Starts `command_line` with the shell.
    pub(crate) fn run(&mut self, command_line: &str) {
        match self.spawn(command_line) {
            Ok(pid) => {
                tracing::debug!(%pid, command_line, "program started");
                self.running
                    .push((pid, self.ledger.track(ResourceKind::Program)));
            }
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                command_line,
                "could not start a program"
            ),
        }
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

    /// Starts `sh -c command_line` as described in the module documentation.
    fn spawn(&self, command_line: &str) -> Result<Pid, SpawnError> {
        let arguments = [
            CString::new("sh").map_err(SpawnError::Nul)?,
            CString::new("-c").map_err(SpawnError::Nul)?,
            CString::new(command_line).map_err(SpawnError::Nul)?,
        ];
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
            Self::SHELL,
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
    /// The command line contains a NUL character.
    #[error("the command line contains a NUL character")]
    Nul(#[source] std::ffi::NulError),
    /// The spawn attributes could not be set up.
    #[error("could not prepare to start the program")]
    Prepare(#[source] nix::Error),
    /// `/dev/null` could not be opened for the program's input and output.
    #[error("could not open /dev/null")]
    OpenNull(#[source] std::io::Error),
    /// The shell could not be started.
    #[error("could not start {}", Launcher::SHELL)]
    Spawn(#[source] nix::Error),
}
