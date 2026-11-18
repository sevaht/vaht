//! The line to vaht: commands the window sends, updates it receives.
//!
//! Talking to vaht blocks, so [`VahtLink`] does it on threads of its own and
//! the window only ever sends a [`Command`] and polls for an [`Update`];
//! [`Link`] is that small interface, so the window is tested against a fake.

use std::sync::mpsc;
use std::thread;

use serde_json::Value;
use vaht_ipc::{Client, ClientError, ConfigEdit, ConfigFile, EventKind, Reply, Request};

/// What the window asks of vaht.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Read the settings again.
    Reload,
    /// Make these edits in place, all or none.
    Edit(Vec<ConfigEdit>),
    /// Write the changes made in place into their files.
    Save,
    /// Drop the changes made in place.
    Revert,
}

/// What vaht told the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// The settings' schema (JSON Schema), sent once, first.
    Schema(Value),
    /// What the settings say now.
    Reading {
        /// The effective values.
        value: Value,
        /// What vaht defaults to.
        defaults: Value,
        /// The keys changed in place and not written.
        changed: Vec<Vec<String>>,
        /// The styles that can be chosen.
        styles: Vec<String>,
        /// What vaht could not use of the configuration files and left
        /// out.
        problems: Vec<String>,
    },
    /// An edit, save or revert was refused, for this reason.
    Refused(String),
    /// A save went through.
    Saved,
    /// vaht cannot be reached: the reason.
    Disconnected(String),
}

/// The line to vaht, as the window uses it.
pub trait Link {
    /// Passes `command` on to vaht.
    fn send(&self, command: Command);

    /// Returns the next update, if one has arrived.
    fn receive(&self) -> Option<Update>;
}

/// The real line: vaht's IPC socket, served by threads.
#[derive(Debug)]
pub struct VahtLink {
    /// Commands to the thread that talks to vaht.
    commands: mpsc::Sender<Command>,
    /// Updates from it.
    updates: mpsc::Receiver<Update>,
}

impl VahtLink {
    /// Connects to the vaht this runs under, and has `wake` called whenever
    /// an update arrives (to repaint the window).
    ///
    /// # Errors
    ///
    /// Returns why vaht cannot be reached.
    pub fn connect(wake: impl Fn() + Send + Clone + 'static) -> Result<Self, ClientError> {
        let mut requests = Client::connect_from_environment()?;
        let mut events = Client::connect_from_environment()?;
        events.subscribe(vec![EventKind::Config])?;
        let (commands, command_queue) = mpsc::channel();
        let (update_sender, updates) = mpsc::channel();
        // A new configuration is news to follow, whoever made it.
        let reload = commands.clone();
        let hears = wake.clone();
        let _events = thread::Builder::new()
            .name("vaht-settings events".to_owned())
            .spawn(move || {
                while events.next_event().is_ok() {
                    if reload.send(Command::Reload).is_err() {
                        return;
                    }
                    hears();
                }
            });
        let _requests = thread::Builder::new()
            .name("vaht-settings requests".to_owned())
            .spawn(move || {
                let mut has_schema = false;
                for command in command_queue {
                    let sent = Self::serve(&mut requests, &command, &mut has_schema)
                        .into_iter()
                        .all(|update| update_sender.send(update).is_ok());
                    wake();
                    if !sent {
                        return;
                    }
                }
            });
        Ok(Self { commands, updates })
    }

    /// Carries out `command` and returns what the window should be told.
    fn serve(client: &mut Client, command: &Command, has_schema: &mut bool) -> Vec<Update> {
        let mut updates = Vec::new();
        let outcome = match command {
            Command::Reload => Ok(()),
            Command::Edit(edits) => Self::done(client.request(&Request::ConfigEdit {
                file: ConfigFile::Settings,
                edits: edits.clone(),
            })),
            Command::Save => Self::done(client.request(&Request::ConfigSave {})).inspect(|()| {
                updates.push(Update::Saved);
            }),
            Command::Revert => Self::done(client.request(&Request::ConfigRevert {
                file: ConfigFile::Settings,
            })),
        };
        if let Err(message) = outcome {
            updates.push(Update::Refused(message));
        }
        match Self::read(client, has_schema) {
            Ok(read) => updates.splice(0..0, read),
            Err(message) => {
                updates.push(Update::Disconnected(message));
                return updates;
            }
        };
        updates
    }

    /// Returns what a reply to an edit, save or revert says: nothing if it
    /// was done, the reason if not.
    fn done(reply: Result<Reply, ClientError>) -> Result<(), String> {
        match reply {
            Ok(Reply::Done {}) => Ok(()),
            Ok(Reply::Error { message }) => Err(message),
            Ok(other) => Err(format!("vaht answered something else: {other:?}")),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Reads the settings (and the schema, the first time).
    fn read(client: &mut Client, has_schema: &mut bool) -> Result<Vec<Update>, String> {
        let mut updates = Vec::new();
        if !*has_schema {
            match client.request(&Request::ConfigSchema {
                file: ConfigFile::Settings,
            }) {
                Ok(Reply::ConfigSchema { schema }) => {
                    *has_schema = true;
                    updates.push(Update::Schema(schema));
                }
                Ok(other) => return Err(format!("vaht answered something else: {other:?}")),
                Err(error) => return Err(error.to_string()),
            }
        }
        let reading = client.request(&Request::ConfigRead {
            file: ConfigFile::Settings,
        });
        let styles = client.request(&Request::ConfigFiles {});
        match (reading, styles) {
            (
                Ok(Reply::ConfigRead {
                    value,
                    defaults,
                    changed,
                }),
                Ok(Reply::ConfigFiles {
                    styles, problems, ..
                }),
            ) => updates.push(Update::Reading {
                value,
                defaults,
                changed,
                styles,
                problems,
            }),
            (Err(error), _) | (_, Err(error)) => return Err(error.to_string()),
            (Ok(other), _) => return Err(format!("vaht answered something else: {other:?}")),
        }
        Ok(updates)
    }
}

impl Link for VahtLink {
    fn send(&self, command: Command) {
        // The thread ends only when vaht does, which the window learns of
        // from an update; a command sent meanwhile has nowhere to go.
        let _ = self.commands.send(command);
    }

    fn receive(&self) -> Option<Update> {
        self.updates.try_recv().ok()
    }
}
