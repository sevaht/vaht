//! The command line, as a typed model.
//!
//! The doc comments on these types become `--help`.

use std::path::PathBuf;

use vaht_compositor::{BackendChoice, RunOptions};

use crate::logging::{LoggingArguments, PROGRAM_NAME};

/// A stacking Wayland compositor descended from Fluxbox.
///
/// Without a subcommand, runs the compositor.
#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct Arguments {
    /// What to do instead of running the compositor.
    #[command(subcommand)]
    pub command: Option<Subcommand>,

    /// Where vaht shows its frames.
    #[arg(long, env = "VAHT_BACKEND", value_enum, default_value_t)]
    pub backend: BackendArgument,

    /// Does not start Xwayland, whatever the settings say: no X11
    /// programs.
    #[arg(long)]
    pub no_xwayland: bool,

    /// The directory of the configuration files; by default
    /// `$XDG_CONFIG_HOME/vaht`, normally `~/.config/vaht`.
    #[arg(long, env = "VAHT_CONFIG_DIRECTORY", global = true)]
    pub config_directory: Option<PathBuf>,

    /// How much diagnostic output to record, and where.
    #[command(flatten)]
    pub logging: LoggingArguments,
}

impl Arguments {
    /// Returns how the compositor should run.
    #[must_use]
    pub fn run_options(&self) -> RunOptions {
        RunOptions {
            backend: self.backend.into(),
            config_directory: self.config_directory(),
            // Found on PATH, if the running program cannot be.
            executable: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(PROGRAM_NAME)),
            allows_xwayland: !self.no_xwayland,
            // The log file's, which the logging set up holds.
            log_rotation: None,
        }
    }

    /// Returns the configuration directory: the one given, or the XDG one.
    /// Without a home directory to put it in, it is a relative `vaht`, which
    /// most likely does not exist, so the built-in configuration applies.
    #[must_use]
    pub fn config_directory(&self) -> PathBuf {
        self.config_directory.clone().unwrap_or_else(|| {
            xdg::BaseDirectories::with_prefix(PROGRAM_NAME)
                .get_config_home()
                .unwrap_or_else(|| PathBuf::from(PROGRAM_NAME))
        })
    }
}

/// The subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Checks the configuration files without running vaht, and says what
    /// is wrong with them.
    Check {
        /// Prints vaht's built-in configuration files instead: what applies
        /// when a file does not exist.
        #[arg(long)]
        print_defaults: bool,
    },
    /// Converts another window manager's configuration to vaht's.
    Import {
        /// Whose configuration.
        #[command(subcommand)]
        source: ImportSource,
    },
    /// Shows one of vaht's dialogs: a window with a line to type in. vaht
    /// runs these itself (the `run-dialog` action), but they work from a
    /// terminal too.
    Dialog {
        /// Which dialog.
        #[command(subcommand)]
        dialog: DialogKind,
    },
    /// Asks the running vaht something, or tells it what to do, over its
    /// IPC socket (`$VAHT_SOCKET`, or the one beside `$WAYLAND_DISPLAY`).
    Msg {
        /// Prints vaht's reply as the JSON it sent, on one line.
        #[arg(long)]
        json: bool,
        /// What to ask.
        #[command(subcommand)]
        request: MessageRequest,
    },
    /// Prints the JSON Schema of the IPC protocol's messages: requests,
    /// replies and events.
    Schema,
}

/// The dialogs `vaht dialog` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::Subcommand)]
pub enum DialogKind {
    /// A command line to run, with Tab completing program and file names
    /// and Up and Down going through the lines run before.
    Run,
    /// A vaht command to run, written as a binding writes one.
    Command,
    /// A window's title, to change; left empty, the window's program's
    /// title comes back.
    Title {
        /// The window, by id (see `vaht msg windows`).
        #[arg(long)]
        window: u64,
    },
}

/// What `vaht msg` asks.
#[derive(Debug, clap::Subcommand)]
pub enum MessageRequest {
    /// Prints the running vaht's version.
    Version,
    /// Runs a command, written as a binding writes one: `vaht msg run
    /// workspace 2`.
    Run {
        /// The window it acts on, by id (see `vaht msg windows`); the
        /// focused window by default.
        #[arg(long)]
        window: Option<u64>,
        /// The command and its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Lists the windows, topmost first: id, workspace (`*` for every
    /// one), state, application id and title.
    Windows,
    /// Lists the workspaces, the shown one marked `*`.
    Workspaces,
    /// Lists the outputs, the primary one marked `*` and those off `-`,
    /// each with matches to paste into `outputs.toml`.
    Outputs,
    /// Prints the lists named now, and again whenever they change, until
    /// vaht stops: `vaht msg subscribe windows workspaces`.
    Subscribe {
        /// The lists.
        #[arg(required = true, value_enum)]
        events: Vec<EventList>,
    },
}

/// A list `vaht msg subscribe` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EventList {
    /// The windows, as `vaht msg windows` lists them.
    Windows,
    /// The workspaces, as `vaht msg workspaces` lists them.
    Workspaces,
    /// The outputs, as `vaht msg outputs` lists them.
    Outputs,
}

impl EventList {
    /// Returns the list as the protocol names it.
    #[must_use]
    pub const fn kind(self) -> vaht_ipc::EventKind {
        match self {
            Self::Windows => vaht_ipc::EventKind::Windows,
            Self::Workspaces => vaht_ipc::EventKind::Workspaces,
            Self::Outputs => vaht_ipc::EventKind::Outputs,
        }
    }
}

impl MessageRequest {
    /// Returns the request as the protocol writes it.
    #[must_use]
    pub fn to_request(&self) -> vaht_ipc::Request {
        match self {
            Self::Version => vaht_ipc::Request::Version {},
            Self::Run { window, command } => vaht_ipc::Request::Run {
                command: command.join(" "),
                window: *window,
            },
            Self::Windows => vaht_ipc::Request::Windows {},
            Self::Workspaces => vaht_ipc::Request::Workspaces {},
            Self::Outputs => vaht_ipc::Request::Outputs {},
            Self::Subscribe { events } => vaht_ipc::Request::Subscribe {
                events: events.iter().map(|list| list.kind()).collect(),
            },
        }
    }
}

/// The configurations `vaht import` converts.
#[derive(Debug, clap::Subcommand)]
pub enum ImportSource {
    /// A Fluxbox setup (`init`, `keys`), or a fluxland one: the same format.
    Fluxbox {
        /// The setup's directory [default: ~/.fluxbox].
        #[arg(long)]
        from: Option<PathBuf>,
        /// Where to write vaht's files [default: the configuration
        /// directory].
        #[arg(long)]
        to: Option<PathBuf>,
        /// Replaces vaht files that exist already.
        #[arg(long)]
        force: bool,
    },
}

/// The `--backend` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum BackendArgument {
    /// In a window of the running compositor, when there is one; else on
    /// the hardware.
    #[default]
    Automatic,
    /// On the hardware, from a virtual terminal.
    Drm,
    /// Nowhere: frames are drawn in memory. For tests.
    Headless,
}

impl From<BackendArgument> for BackendChoice {
    fn from(argument: BackendArgument) -> Self {
        match argument {
            BackendArgument::Automatic => Self::Automatic,
            BackendArgument::Drm => Self::Drm,
            BackendArgument::Headless => Self::Headless,
        }
    }
}
