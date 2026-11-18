//! What the compositor answers each IPC request with.

use vaht_ipc::{
    ConfigEdit, ConfigFile, ConfigFileInfo, ConfigState, Event, EventKind, ModeInfo, OutputInfo,
    Reply, Request, WindowInfo, WorkspaceInfo,
};

use super::{Answer, IpcClient, IpcHandler, IpcServer};
use crate::Compositor;
use crate::WindowId;
use crate::config::{Action, Command, LayoutFile, SettingChange, StyleFile, schema};
use crate::execution::ExecutionContext;
use crate::outputs::Outputs;

impl IpcHandler for Compositor {
    fn answer(&mut self, request: Request, client: IpcClient) -> Answer {
        if let Request::Pick { title, items } = request {
            return self.start_pick(client.number, title, items);
        }
        Answer::Now(self.answer_now(request, client.peer))
    }
}

impl Compositor {
    /// Returns the refusal of a command from the process `peer`, if
    /// `[ipc] allow_commands` is off and it is not one of vaht's own
    /// dialogs (the settings window among them).
    fn refuse_commands_from(&self, peer: Option<i32>) -> Option<Reply> {
        let is_own_dialog = peer.is_some_and(|pid| self.launcher.is_dialog(pid));
        (!self.config.settings.ipc.allow_commands && !is_own_dialog).then(|| Reply::Error {
            message: "commands from other programs are turned off ([ipc] allow_commands)"
                .to_owned(),
        })
    }

    /// Makes `edits` to `file` in place, all or none.
    fn edit_config(&mut self, file: ConfigFile, edits: &[ConfigEdit]) -> Result<(), String> {
        match file {
            ConfigFile::Settings => {
                let changes = edits
                    .iter()
                    .map(SettingChange::of_edit)
                    .collect::<Result<Vec<_>, _>>()?;
                self.change_settings(changes)
            }
            ConfigFile::Outputs | ConfigFile::Bindings | ConfigFile::Rules | ConfigFile::Menus => {
                Err(format!(
                    "{} cannot be edited through IPC yet",
                    file.file_name()
                ))
            }
        }
    }

    /// Puts `file` back to what it says.
    fn revert_config(&mut self, file: ConfigFile) -> Result<(), String> {
        match file {
            ConfigFile::Settings => {
                self.revert_settings();
                Ok(())
            }
            ConfigFile::Outputs | ConfigFile::Bindings | ConfigFile::Rules | ConfigFile::Menus => {
                Err(format!(
                    "{} cannot be edited through IPC yet",
                    file.file_name()
                ))
            }
        }
    }

    /// Returns each file that can be read, with whether it has changes not
    /// written.
    fn config_file_infos(&self) -> Vec<ConfigFileInfo> {
        vec![ConfigFileInfo {
            file: ConfigFile::Settings,
            state: if self.overrides.is_empty() {
                ConfigState::Clean
            } else {
                ConfigState::Changed
            },
        }]
    }

    /// Returns what could not be used of the configuration files, a sentence
    /// each.
    fn config_problem_texts(&self) -> Vec<String> {
        self.config_banner
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Returns the error for a file that cannot be described yet.
    fn undescribed(file: ConfigFile) -> Reply {
        Reply::Error {
            message: format!("{} cannot be read through IPC yet", file.file_name()),
        }
    }

    /// Carries out `request`, from the process `peer` (none if unknown),
    /// and returns the reply.
    fn answer_now(&mut self, request: Request, peer: Option<i32>) -> Reply {
        match request {
            Request::Version {} => Reply::Version {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            Request::Run { command, window } => {
                if let Some(refused) = self.refuse_commands_from(peer) {
                    return refused;
                }
                let action = match Action::parse(&command) {
                    Ok(action) => action,
                    Err(message) => return Reply::Error { message },
                };
                let window = match window.map(WindowId::new) {
                    Some(id) if self.window_manager.window(id).is_none() => {
                        return Reply::Error {
                            message: format!("there is no window {}", id.get()),
                        };
                    }
                    Some(id) => Some(id),
                    None => self.window_manager.focused(),
                };
                tracing::debug!(command, "running a command for an IPC client");
                self.execute(
                    &Command::Run(action),
                    &mut ExecutionContext::for_window(window),
                );
                Reply::Done {}
            }
            Request::Windows {} => Reply::Windows {
                windows: self.window_infos(),
            },
            Request::Workspaces {} => Reply::Workspaces {
                workspaces: self.workspace_infos(),
            },
            Request::Outputs {} => Reply::Outputs {
                outputs: output_infos(&self.outputs),
                profile: self.outputs.profile().map(str::to_owned),
            },
            Request::LogFilter { directives, reset } => match &self.log_filter {
                Some(filter) => match filter.change(directives.as_deref(), reset) {
                    Ok(now) => {
                        if reset || directives.is_some() {
                            tracing::info!(directives = now, "log filter changed");
                        }
                        Reply::LogFilter { directives: now }
                    }
                    Err(message) => Reply::Error { message },
                },
                None => Reply::Error {
                    message: "this vaht has no log filter to change".to_owned(),
                },
            },
            Request::ConfigFiles {} => Reply::ConfigFiles {
                files: self.config_file_infos(),
                styles: StyleFile::available(self.config_source.directory()),
                layouts: LayoutFile::available(self.config_source.directory()),
                problems: self.config_problem_texts(),
            },
            Request::ConfigSchema { file } => match schema::schema_of(file) {
                Some(schema) => Reply::ConfigSchema { schema },
                None => Self::undescribed(file),
            },
            Request::ConfigRead { file } => match file {
                ConfigFile::Settings => Reply::ConfigRead {
                    value: schema::settings_value(&self.config.settings),
                    defaults: schema::settings_value(&self.config.layout.defaults),
                    changed: self.overrides.keys(),
                    layout: self.config.layout.name.clone(),
                    from_layout: self.config.layout.keys.clone(),
                },
                ConfigFile::Outputs
                | ConfigFile::Bindings
                | ConfigFile::Rules
                | ConfigFile::Menus => Self::undescribed(file),
            },
            Request::ConfigEdit { file, edits } => {
                if let Some(refused) = self.refuse_commands_from(peer) {
                    return refused;
                }
                self.edit_config(file, &edits)
                    .map_or_else(|message| Reply::Error { message }, |()| Reply::Done {})
            }
            Request::ConfigSave {} => {
                if let Some(refused) = self.refuse_commands_from(peer) {
                    return refused;
                }
                self.save_settings()
                    .map_or_else(|message| Reply::Error { message }, |()| Reply::Done {})
            }
            Request::ConfigRevert { file } => {
                if let Some(refused) = self.refuse_commands_from(peer) {
                    return refused;
                }
                self.revert_config(file)
                    .map_or_else(|message| Reply::Error { message }, |()| Reply::Done {})
            }
            // A connection answers this itself, and a pick comes later.
            Request::Subscribe { .. } | Request::Pick { .. } => Reply::Error {
                message: "subscriptions are kept by the connection".to_owned(),
            },
        }
    }
}

impl Compositor {
    /// Returns the list `kind` as it is now, for clients subscribed to it.
    pub(crate) fn ipc_event(&self, kind: EventKind) -> Event {
        match kind {
            EventKind::Windows => Event::Windows {
                windows: self.window_infos(),
            },
            EventKind::Workspaces => Event::Workspaces {
                workspaces: self.workspace_infos(),
            },
            EventKind::Outputs => Event::Outputs {
                outputs: output_infos(&self.outputs),
                profile: self.outputs.profile().map(str::to_owned),
            },
            EventKind::Config => Event::Config {
                generation: self.config_generation,
                files: self.config_file_infos(),
                problems: self.config_problem_texts(),
            },
        }
    }

    /// Sends each list some client subscribed to, to those whose copy is
    /// out of date.
    pub(crate) fn publish_ipc_events(&mut self) {
        let Some(kinds) = self.ipc.as_ref().map(IpcServer::subscribed) else {
            return;
        };
        for kind in kinds {
            let event = self.ipc_event(kind);
            if let Some(server) = &mut self.ipc {
                server.publish(&event);
            }
        }
    }

    /// Describes each window for IPC, topmost first.
    fn window_infos(&self) -> Vec<WindowInfo> {
        let focused = self.window_manager.focused();
        self.window_manager
            .windows_top_down()
            .map(|window| {
                let state = window.state();
                WindowInfo {
                    id: window.id().get(),
                    title: window.title().to_owned(),
                    app_id: window.app_id().to_owned(),
                    workspace: (!state.is_sticky).then(|| window.workspace().number().get()),
                    is_focused: focused == Some(window.id()),
                    is_minimized: state.is_minimized,
                    is_shaded: state.is_shaded,
                    parent: window.parent().map(WindowId::get),
                }
            })
            .collect()
    }

    /// Describes each workspace for IPC, in order.
    fn workspace_infos(&self) -> Vec<WorkspaceInfo> {
        let current = self.window_manager.current_workspace();
        self.window_manager
            .workspaces()
            .map(|(workspace, name)| WorkspaceInfo {
                number: workspace.number().get(),
                name: name.to_owned(),
                is_current: workspace == current,
            })
            .collect()
    }
}

/// Describes each of `outputs`, on or off, for IPC.
fn output_infos(outputs: &Outputs) -> Vec<OutputInfo> {
    outputs
        .connected()
        .map(|output| {
            let identity = Outputs::identity(output);
            let is_enabled = outputs.iter().any(|on| on == output);
            let location = output.current_location();
            OutputInfo {
                connector: identity.connector,
                make: identity.make,
                model: identity.model,
                serial: identity.serial,
                is_enabled,
                is_primary: output == outputs.primary(),
                mode: output
                    .current_mode()
                    .filter(|_| is_enabled)
                    .map(|mode| ModeInfo {
                        width: mode.size.w,
                        height: mode.size.h,
                        refresh_millihertz: mode.refresh,
                    }),
                position: [location.x, location.y],
                scale: output.current_scale().fractional_scale(),
            }
        })
        .collect()
}
