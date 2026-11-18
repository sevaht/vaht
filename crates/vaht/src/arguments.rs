//! The command line, as a typed model.
//!
//! The doc comments on these types become `--help`.

use std::path::PathBuf;

use vaht_compositor::{BackendChoice, RunOptions};

use crate::logging::{LoggingArguments, PROGRAM_NAME};

/// vaht's version and the commit it was built from (`build.rs`), as
/// `vaht --version` prints them: `0.1.0 (1a2b3c4d5e6f)`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("VAHT_COMMIT"), ")");

/// A stacking Wayland compositor descended from Fluxbox.
///
/// Without a subcommand, runs the compositor. `version` and the commands
/// after it, and `config files|get|set|unset|save|revert`, ask the running
/// vaht over its IPC socket (`$VAHT_SOCKET`, or the one beside
/// `$WAYLAND_DISPLAY`) and fail if none runs; the others need no running vaht.
#[derive(Debug, clap::Parser)]
#[command(version = VERSION)]
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

    /// For the commands that ask the running vaht: prints its reply as the
    /// JSON it sent, on one line.
    #[arg(long, global = true)]
    pub json: bool,

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
            // The logging's, which it hands over once set up.
            log_rotation: None,
            log_filter: None,
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
///
/// `config`, `import`, `dialog` and `ipc-schema` are about vaht's own files
/// and programs; the rest ([`LiveCommand`]) ask the running vaht something,
/// or tell it what to do, over its IPC socket (`$VAHT_SOCKET`, or the one
/// beside `$WAYLAND_DISPLAY`).
#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// The configuration: checks the files, and reads and changes the
    /// settings of the running vaht.
    Config {
        /// What to do.
        #[command(subcommand)]
        command: ConfigCommand,
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
    /// Prints the JSON Schema of the IPC protocol's messages: requests,
    /// replies and events.
    IpcSchema,
    /// What the running vaht is asked.
    #[command(flatten)]
    Live(LiveCommand),
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
        /// The window, by id (see `vaht windows`).
        #[arg(long)]
        window: u64,
    },
}

/// What the running vaht is asked: each is one request over its IPC socket.
#[derive(Debug, clap::Subcommand)]
pub enum LiveCommand {
    /// Prints the running vaht's version.
    Version,
    /// Runs a command, written as a binding writes one: `vaht run
    /// workspace 2`.
    Run {
        /// The window it acts on, by id (see `vaht windows`); the
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
    /// vaht stops: `vaht subscribe windows workspaces`.
    Subscribe {
        /// The lists.
        #[arg(required = true, value_enum)]
        events: Vec<EventList>,
    },
    /// Prints which events vaht records, or changes it while vaht runs:
    /// `vaht log-filter info,vaht_compositor::input=trace`, or `vaht
    /// log-filter reset` for the filter vaht started with.
    LogFilter {
        /// Tracing filter directives, or `reset`; none prints the filter.
        directives: Option<String>,
    },
    /// Shows the lines read from standard input as a vaht menu at the
    /// pointer and prints the one the user chooses; exits with 1, printing
    /// nothing, if they close the menu. A dmenu-style chooser: the
    /// screen-share portal runs `vaht pick --title Share`.
    Pick {
        /// The menu's title.
        #[arg(long)]
        title: Option<String>,
    },
}

/// What `vaht config` does. `check` and `schema` need no running vaht; the
/// rest ask the running one.
#[derive(Debug, Clone, PartialEq, Eq, clap::Subcommand)]
pub enum ConfigCommand {
    /// Checks the configuration files without running vaht, and lists
    /// everything wrong with them.
    Check {
        /// Prints vaht's built-in configuration files instead: what applies
        /// when a file does not exist.
        #[arg(long)]
        print_defaults: bool,
    },
    /// Prints a file's schema (JSON Schema): every key with its type,
    /// documentation, default and range.
    Schema {
        /// The file.
        #[arg(value_enum, default_value_t)]
        file: ConfigFileArgument,
    },
    /// Lists the files that can be read, whether each has changes not
    /// written, the styles there are, and what vaht left out of the files
    /// because it could not use it.
    Files,
    /// Prints what a file says now, a `key = value` line each, the keys
    /// changed in place and not written marked `*`.
    Get {
        /// The file.
        #[arg(value_enum, default_value_t)]
        file: ConfigFileArgument,
    },
    /// Changes a setting in place, until `save` writes it: `vaht config
    /// set blur.spread 7`. The value is JSON (`true`, `7`,
    /// `"sloppy"`, `[1, 2]`), or a bare word taken as a string. Nothing
    /// changes if the whole configuration would not be valid.
    Set {
        /// The setting, such as `focus.model` or `input.device."My
        /// Mouse".accel_speed`.
        #[arg(value_parser = parse_config_path)]
        key: vaht_ipc::ConfigPath,
        /// Its new value.
        value: String,
        /// The file.
        #[arg(long, value_enum, default_value_t)]
        file: ConfigFileArgument,
    },
    /// Puts a setting back to its default, in place, until `save` leaves
    /// it out of the file.
    Unset {
        /// The setting.
        #[arg(value_parser = parse_config_path)]
        key: vaht_ipc::ConfigPath,
        /// The file.
        #[arg(long, value_enum, default_value_t)]
        file: ConfigFileArgument,
    },
    /// Writes the changes made in place into their files, keeping the
    /// files' comments and layout.
    Save,
    /// Drops the changes made in place: a file runs as it says.
    Revert {
        /// The file.
        #[arg(value_enum, default_value_t)]
        file: ConfigFileArgument,
    },
}

/// Reads a setting's key for `vaht config`.
fn parse_config_path(text: &str) -> Result<vaht_ipc::ConfigPath, String> {
    vaht_ipc::ConfigPath::parse(text).map_err(|error| error.to_string())
}

/// A configuration file `vaht config` is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ConfigFileArgument {
    /// `settings.toml`.
    #[default]
    Settings,
    /// `outputs.toml`.
    Outputs,
    /// `bindings.toml`.
    Bindings,
    /// `rules.toml`.
    Rules,
    /// `menus.toml`.
    Menus,
}

impl ConfigFileArgument {
    /// Returns the file as the protocol names it.
    #[must_use]
    pub const fn file(self) -> vaht_ipc::ConfigFile {
        match self {
            Self::Settings => vaht_ipc::ConfigFile::Settings,
            Self::Outputs => vaht_ipc::ConfigFile::Outputs,
            Self::Bindings => vaht_ipc::ConfigFile::Bindings,
            Self::Rules => vaht_ipc::ConfigFile::Rules,
            Self::Menus => vaht_ipc::ConfigFile::Menus,
        }
    }
}

/// A list `vaht subscribe` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum EventList {
    /// The windows, as `vaht windows` lists them.
    Windows,
    /// The workspaces, as `vaht workspaces` lists them.
    Workspaces,
    /// The outputs, as `vaht outputs` lists them.
    Outputs,
    /// The configuration: each file and whether it has changes not written,
    /// whenever vaht runs a new configuration.
    Config,
}

impl EventList {
    /// Returns the list as the protocol names it.
    #[must_use]
    pub const fn kind(self) -> vaht_ipc::EventKind {
        match self {
            Self::Windows => vaht_ipc::EventKind::Windows,
            Self::Workspaces => vaht_ipc::EventKind::Workspaces,
            Self::Outputs => vaht_ipc::EventKind::Outputs,
            Self::Config => vaht_ipc::EventKind::Config,
        }
    }
}

impl ConfigCommand {
    /// Returns the request as the protocol writes it; none for what needs no
    /// running vaht (`check`, `schema`).
    #[must_use]
    pub fn to_request(&self) -> Option<vaht_ipc::Request> {
        Some(match self {
            Self::Check { .. } | Self::Schema { .. } => return None,
            Self::Files => vaht_ipc::Request::ConfigFiles {},
            Self::Get { file } => vaht_ipc::Request::ConfigRead { file: file.file() },
            Self::Set { key, value, file } => vaht_ipc::Request::ConfigEdit {
                file: file.file(),
                edits: vec![vaht_ipc::ConfigEdit::Set {
                    path: key.clone(),
                    // JSON where it is, else the bare word.
                    value: serde_json::from_str(value)
                        .unwrap_or_else(|_| serde_json::Value::String(value.clone())),
                }],
            },
            Self::Unset { key, file } => vaht_ipc::Request::ConfigEdit {
                file: file.file(),
                edits: vec![vaht_ipc::ConfigEdit::Unset { path: key.clone() }],
            },
            Self::Save => vaht_ipc::Request::ConfigSave {},
            Self::Revert { file } => vaht_ipc::Request::ConfigRevert { file: file.file() },
        })
    }
}

impl LiveCommand {
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
            Self::LogFilter { directives } => {
                let reset = directives.as_deref() == Some("reset");
                vaht_ipc::Request::LogFilter {
                    directives: directives.clone().filter(|_| !reset),
                    reset,
                }
            }
            // The items come from standard input ([`crate::message::pick`]).
            Self::Pick { title } => vaht_ipc::Request::Pick {
                title: title.clone(),
                items: Vec::new(),
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
    /// Fluxbox styles (colors, fonts, textures), converted to vaht styles on
    /// their own: nothing else of a setup is read or written. Each is a style
    /// file, a style directory (holding `theme.cfg`), or a directory of
    /// styles, searched through: `vaht import fluxbox-style
    /// /usr/share/fluxbox/styles`.
    FluxboxStyle {
        /// What to convert; any number.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// The directory to write the styles into, each as `<name>.toml`
        /// [default: `styles` in the configuration directory].
        #[arg(long)]
        to: Option<PathBuf>,
        /// Replaces styles that exist already.
        #[arg(long)]
        force: bool,
        /// Prints every line not converted of every style (one style prints
        /// them anyway; several print how many).
        #[arg(long)]
        notes: bool,
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
