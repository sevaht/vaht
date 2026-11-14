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
//! - with no input, and its output discarded; a dialog's errors are read
//!   from a pipe instead and logged, since nothing else would show them.
//!
//! Every program is kept, and counted, until it has exited and been reaped:
//! vaht is told of exits by SIGCHLD (see `signals`), so no zombie outlives
//! its program.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::Read as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::event_source::EventSource;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::session_environment::TOOLKIT_DEFAULTS;
use crate::{Compositor, WindowId};

/// Starts programs and reaps them.
#[derive(Debug)]
pub(crate) struct Launcher {
    /// The programs started that have not been reaped yet.
    running: Vec<(Pid, Tracked)>,
    /// Which of them are vaht's own dialogs, which may always run commands
    /// over IPC.
    dialogs: Vec<Pid>,
    /// The error output of dialogs still writing it, by process.
    error_outputs: BTreeMap<i32, EventSource<Compositor>>,
    /// Dialogs whose error output ended during the last dispatch: their
    /// sources go after it, never from inside their own callback.
    ended_outputs: Vec<i32>,
    /// Registers the error outputs' sources.
    loop_handle: LoopHandle<'static, Compositor>,
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

    /// The desktop's name, as `XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`
    /// and the session file's `DesktopNames` give it.
    const DESKTOP_NAME: &str = "vaht";

    /// Creates a launcher whose programs connect to `socket_name`, and
    /// whose dialogs are shown by `executable`; their errors are read on
    /// `loop_handle`'s loop.
    pub(crate) fn new(
        socket_name: OsString,
        executable: PathBuf,
        loop_handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
    ) -> Self {
        Self {
            running: Vec::new(),
            dialogs: Vec::new(),
            error_outputs: BTreeMap::new(),
            ended_outputs: Vec::new(),
            loop_handle: loop_handle.clone(),
            environment: BTreeMap::from([
                (OsString::from("WAYLAND_DISPLAY"), socket_name),
                // Portals and toolkits choose by these: vaht's own
                // vaht-portals.conf, and Wayland rather than X11.
                (
                    OsString::from("XDG_CURRENT_DESKTOP"),
                    OsString::from(Self::DESKTOP_NAME),
                ),
                (
                    OsString::from("XDG_SESSION_DESKTOP"),
                    OsString::from(Self::DESKTOP_NAME),
                ),
                (
                    OsString::from("XDG_SESSION_TYPE"),
                    OsString::from("wayland"),
                ),
            ]),
            executable,
            ledger: ledger.clone(),
        }
        .with_toolkit_defaults()
    }

    /// Returns the launcher with toolkits told to prefer Wayland, where
    /// vaht's own environment does not say otherwise.
    fn with_toolkit_defaults(mut self) -> Self {
        for (name, value) in TOOLKIT_DEFAULTS {
            if std::env::var_os(name).is_none() {
                self.environment
                    .insert(OsString::from(name), OsString::from(value));
            }
        }
        self
    }

    /// Returns whether the programs vaht starts find `name` set.
    pub(crate) fn is_set(&self, name: &str) -> bool {
        self.environment.contains_key(OsStr::new(name)) || std::env::var_os(name).is_some()
    }

    /// Starts `command_line` with the shell.
    pub(crate) fn run(&mut self, command_line: &str) {
        let arguments = ["sh", "-c", command_line].map(OsStr::new);
        if let Err(error) = self.start(Path::new(Self::SHELL), &arguments, command_line, None) {
            Self::warn_not_started(&error, command_line);
        }
    }

    /// Starts `first` with the shell, and each of `then` once it is done,
    /// as the autostart programs follow the activation environment's
    /// update: services they start must find the session.
    pub(crate) fn run_in_order(&mut self, first: &str, then: &[String]) {
        let script = format!("{first}\nfor line do sh -c \"$line\" & done");
        let arguments: Vec<&OsStr> = ["sh", "-c", &script, "sh"]
            .into_iter()
            .chain(then.iter().map(String::as_str))
            .map(OsStr::new)
            .collect();
        if let Err(error) = self.start(Path::new(Self::SHELL), &arguments, first, None) {
            Self::warn_not_started(&error, first);
        }
    }

    /// Starts `command_line` with the shell, given `argument` as its
    /// first argument (`$1`), as hooks are.
    pub(crate) fn run_hook(&mut self, command_line: &str, argument: &str) {
        let script = format!("{command_line} \"$1\"");
        let arguments = ["sh", "-c", &script, "sh", argument].map(OsStr::new);
        if let Err(error) = self.start(Path::new(Self::SHELL), &arguments, command_line, None) {
            Self::warn_not_started(&error, command_line);
        }
    }

    /// Returns whether the process `pid` is one of vaht's dialogs, still
    /// running.
    pub(crate) fn is_dialog(&self, pid: i32) -> bool {
        self.dialogs.contains(&Pid::from_raw(pid))
    }

    /// Drops the sources of the error outputs that ended.
    pub(crate) fn release_ended_outputs(&mut self) {
        for pid in self.ended_outputs.drain(..) {
            self.error_outputs.remove(&pid);
        }
    }

    /// Shows vaht's dialog `name` (`vaht dialog <name>`), styled by the
    /// configuration in `config_directory`.
    pub(crate) fn show_dialog(&mut self, name: &str, config_directory: &Path) {
        self.start_dialog(name, &[], config_directory);
    }

    /// Shows vaht's dialog `name` for `window` (`vaht dialog <name>
    /// --window <id>`), styled by the configuration in `config_directory`.
    pub(crate) fn show_window_dialog(
        &mut self,
        name: &str,
        window: WindowId,
        config_directory: &Path,
    ) {
        let id = window.get().to_string();
        self.start_dialog(
            name,
            &[OsStr::new("--window"), OsStr::new(&id)],
            config_directory,
        );
    }

    /// Starts `vaht dialog <name> <extra>`, styled by the configuration in
    /// `config_directory`.
    fn start_dialog(&mut self, name: &str, extra: &[&OsStr], config_directory: &Path) {
        let executable = self.executable.clone();
        let mut arguments = vec![
            OsStr::new("vaht"),
            OsStr::new("--config-directory"),
            config_directory.as_os_str(),
            OsStr::new("dialog"),
            OsStr::new(name),
        ];
        arguments.extend_from_slice(extra);
        let what = format!("the {name} dialog");
        let started = std::io::pipe()
            .map_err(SpawnError::Pipe)
            .and_then(|(reader, writer)| {
                // vaht's end of the writing side closes once the dialog has
                // its own, so the pipe ends when the dialog does.
                let writer = std::os::fd::OwnedFd::from(writer);
                let pid = self.start(&executable, &arguments, &what, Some(&writer))?;
                Ok((pid, reader))
            });
        match started {
            Ok((pid, reader)) => {
                self.dialogs.push(pid);
                self.read_errors(pid, reader, name.to_owned());
            }
            Err(error) => Self::warn_not_started(&error, &what),
        }
    }

    /// Logs what the dialog `name`, process `pid`, writes to `reader`, a
    /// line at a time, until it closes it.
    fn read_errors(&mut self, pid: Pid, reader: std::io::PipeReader, name: String) {
        let reader = File::from(std::os::fd::OwnedFd::from(reader));
        let mut pending = Vec::new();
        let source = Generic::new(reader, Interest::READ, Mode::Level);
        let key = pid.as_raw();
        let inserted = EventSource::insert(
            &self.loop_handle,
            &self.ledger,
            source,
            move |_, reader, compositor| {
                let mut chunk = [0; 1024];
                let read = (&mut &**reader).read(&mut chunk);
                let is_ended = match read {
                    Ok(0) | Err(_) => true,
                    Ok(count) => {
                        pending.extend_from_slice(&chunk[..count]);
                        false
                    }
                };
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    let line = String::from_utf8_lossy(&line);
                    tracing::warn!(dialog = name, "{}", line.trim_end());
                }
                if !is_ended {
                    return Ok(PostAction::Continue);
                }
                if !pending.is_empty() {
                    tracing::warn!(dialog = name, "{}", String::from_utf8_lossy(&pending));
                }
                compositor.launcher.ended_outputs.push(key);
                Ok(PostAction::Disable)
            },
        );
        match inserted {
            Ok(source) => {
                self.error_outputs.insert(key, source);
            }
            Err(error) => tracing::warn!(
                error = &error.error as &dyn std::error::Error,
                "a dialog's errors cannot be read"
            ),
        }
    }

    /// Logs that the program `what` could not be started, and why.
    fn warn_not_started(error: &SpawnError, what: &str) {
        tracing::warn!(
            error = error as &dyn std::error::Error,
            what,
            "could not start a program"
        );
    }

    /// Sets `name` to `value` for every program started from now on.
    pub(crate) fn set_environment(&mut self, name: String, value: String) {
        self.environment.insert(name.into(), value.into());
    }

    /// Reaps every program that has exited.
    pub(crate) fn reap(&mut self) {
        let dialogs = &mut self.dialogs;
        self.running
            .retain(|(pid, _)| match waitpid(*pid, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) => true,
                Ok(status) => {
                    tracing::debug!(%pid, ?status, "program exited");
                    // Stopped and continued programs are still running.
                    let is_gone =
                        matches!(status, WaitStatus::Exited(..) | WaitStatus::Signaled(..));
                    if is_gone {
                        dialogs.retain(|dialog| dialog != pid);
                    }
                    !is_gone
                }
                Err(error) => {
                    tracing::warn!(%error, %pid, "could not check on a program; no longer tracking it");
                    dialogs.retain(|dialog| dialog != pid);
                    false
                }
            });
    }

    /// Starts `program` with `arguments` (its own name first), its errors
    /// going to `errors` if given, keeping it until it is reaped; `what`
    /// names it in the log.
    fn start(
        &mut self,
        program: &Path,
        arguments: &[&OsStr],
        what: &str,
        errors: Option<&std::os::fd::OwnedFd>,
    ) -> Result<Pid, SpawnError> {
        let pid = self.spawn(program, arguments, errors)?;
        tracing::debug!(%pid, what, "program started");
        self.running
            .push((pid, self.ledger.track(ResourceKind::Program)));
        Ok(pid)
    }

    /// Starts `program` with `arguments` as described in the module
    /// documentation; its error output goes to `errors` if given.
    fn spawn(
        &self,
        program: &Path,
        arguments: &[&OsStr],
        errors: Option<&std::os::fd::OwnedFd>,
    ) -> Result<Pid, SpawnError> {
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
            let target = match errors {
                Some(errors) if standard == 2 => errors.as_raw_fd(),
                Some(_) | None => null.as_raw_fd(),
            };
            file_actions
                .add_dup2(target, standard)
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
    /// No pipe could be made for a dialog's errors.
    #[error("could not make a pipe for the program's errors")]
    Pipe(#[source] std::io::Error),
    /// `/dev/null` could not be opened for the program's input and output.
    #[error("could not open /dev/null")]
    OpenNull(#[source] std::io::Error),
    /// The program could not be started.
    #[error("could not start the program")]
    Spawn(#[source] nix::Error),
}
