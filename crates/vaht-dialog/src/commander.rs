//! Carrying out vaht commands for a dialog: over IPC, or anything a test
//! puts in its place.

use vaht_ipc::{Client, Reply, Request};

/// Carries out vaht commands.
pub trait Commander {
    /// Runs `command`, written as a binding writes one.
    ///
    /// # Errors
    ///
    /// Returns whether vaht refused the command, or could not be asked.
    fn run(&mut self, command: &str) -> Result<(), CommandError>;
}

/// Why a command was not carried out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// vaht refused it, saying why: an unknown action, say.
    #[error("{0}")]
    Refused(String),
    /// vaht could not be asked.
    #[error("{0}")]
    Unreachable(String),
}

/// Asks the vaht this program runs under, over IPC.
#[derive(Debug, Clone, Copy, Default)]
pub struct IpcCommander;

impl Commander for IpcCommander {
    fn run(&mut self, command: &str) -> Result<(), CommandError> {
        let request = Request::Run {
            command: command.to_owned(),
            window: None,
        };
        let reply = Client::connect_from_environment()
            .and_then(|mut client| client.request(&request))
            .map_err(|error| {
                // The causes, as `{:#}` would show them.
                let mut message = error.to_string();
                let mut source = std::error::Error::source(&error);
                while let Some(cause) = source {
                    message.push_str(": ");
                    message.push_str(&cause.to_string());
                    source = cause.source();
                }
                CommandError::Unreachable(message)
            })?;
        match reply {
            Reply::Done {} => Ok(()),
            Reply::Error { message } => Err(CommandError::Refused(message)),
            other @ (Reply::Version { .. } | Reply::Windows { .. } | Reply::Workspaces { .. }) => {
                Err(CommandError::Unreachable(format!(
                    "vaht answered something else: {other:?}"
                )))
            }
        }
    }
}
