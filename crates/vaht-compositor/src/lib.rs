//! The vaht compositor: windows, outputs, input and rendering, built on Smithay.
//!
//! The design is in `docs/design/01-object-model.md`. One [`Compositor`]
//! state holds the subsystems and routes between them; policy lives in plain
//! types, and the Smithay handler traits in `protocols` are thin gateways.
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
mod execute;
mod focus;
mod grab;
mod input;
mod launcher;
mod protocols;
pub mod resources;
mod scene;
mod shared_memory;
mod signals;
mod window;
mod windows;
mod workspaces;

pub use backend::{BackendChoice, BackendError, RenderError};
pub use compositor::{Compositor, RunError, RunOptions};
pub use config::{Config, ConfigError, ConfigFile};
pub use signals::ProcessSignals;
pub use window::WindowId;
