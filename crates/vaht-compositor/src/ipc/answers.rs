//! What the compositor answers each IPC request with.

use vaht_ipc::{Reply, Request, WindowInfo, WorkspaceInfo};

use super::IpcHandler;
use crate::Compositor;
use crate::WindowId;
use crate::config::{Action, Command};
use crate::execution::ExecutionContext;

impl IpcHandler for Compositor {
    fn answer(&mut self, request: Request, peer: Option<i32>) -> Reply {
        match request {
            Request::Version {} => Reply::Version {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            Request::Run { command, window } => {
                let is_own_dialog = peer.is_some_and(|pid| self.launcher.is_dialog(pid));
                if !self.config.settings.ipc.allow_commands && !is_own_dialog {
                    return Reply::Error {
                        message:
                            "commands from other programs are turned off ([ipc] allow_commands)"
                                .to_owned(),
                    };
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
            Request::Windows {} => {
                let focused = self.window_manager.focused();
                let windows = self
                    .window_manager
                    .windows_top_down()
                    .map(|window| {
                        let state = window.state();
                        WindowInfo {
                            id: window.id().get(),
                            title: window.title().to_owned(),
                            app_id: window.app_id().to_owned(),
                            workspace: (!state.is_sticky)
                                .then(|| window.workspace().number().get()),
                            is_focused: focused == Some(window.id()),
                            is_minimized: state.is_minimized,
                            is_shaded: state.is_shaded,
                        }
                    })
                    .collect();
                Reply::Windows { windows }
            }
            Request::Workspaces {} => {
                let current = self.window_manager.current_workspace();
                let workspaces = self
                    .window_manager
                    .workspaces()
                    .map(|(workspace, name)| WorkspaceInfo {
                        number: workspace.number().get(),
                        name: name.to_owned(),
                        is_current: workspace == current,
                    })
                    .collect();
                Reply::Workspaces { workspaces }
            }
        }
    }
}
