//! The vaht compositor: windows, outputs, input and rendering, built on Smithay.
//!
//! The design is in `docs/design/01-object-model.md`. The window manager
//! (`window_manager`) decides everything about windows, workspaces and
//! focus, with no Wayland in it; one [`Compositor`] state holds it and the
//! other subsystems, routes events between them and carries its decisions
//! out through Smithay, whose handler traits in `protocols` are thin
//! gateways.
//!
//! Today vaht runs nested in a window of another compositor: it accepts
//! clients, shows their windows stacked, focuses and raises on click, and
//! routes keyboard and pointer input. The foundations every subsystem uses
//! are public: counted resources ([`resources`]) and self-removing event-loop
//! registrations ([`event_source`]).

mod backend;
mod capture;
mod client;
mod compositor;
mod config;
pub mod event_source;
mod execution;
mod grab;
mod input;
mod launcher;
mod layers;
mod protocols;
pub mod resources;
mod scene;
mod shared_memory;
mod signals;
mod toplevels;
mod window_manager;

pub use backend::{BackendChoice, BackendError, RenderError};
pub use client::ListenError;
pub use compositor::{Compositor, RunError, RunOptions};
pub use config::{Config, ConfigError, ConfigFile};
pub use signals::ProcessSignals;
pub use window_manager::WindowId;
