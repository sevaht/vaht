//! Drives a headless vaht from tests: real clients, pointer input and
//! screenshots.
//!
//! [`HeadlessVaht`] runs the `vaht` executable with `--backend headless` and
//! private runtime, configuration and log directories, and reads its log and
//! exit report. A
//! [`TestClient`] connects to it like any Wayland program: it opens
//! solid-colored windows, moves and clicks a virtual pointer, types on a
//! virtual keyboard, and takes
//! [`Screenshot`]s through the same capture protocol `grim` uses. Nothing
//! reaches the developer's own session.

mod client;
mod clipboard;
mod gamma_control;
mod headless;
mod input_method;
mod metrics;
mod output_configurator;
mod output_power;
mod pager;
mod picture;
mod sandbox;
mod screenshot;
mod toplevel_manager;
mod wait;
mod window_capture;
mod window_list;
mod x11_client;

pub use client::{
    BUTTON_LEFT, BUTTON_RIGHT, DragEnd, KeyboardFocus, OUTSIDE_COLOR, Presentation, TestClient, TestLayer,
    WINDOW_COLOR, WINDOW_SIZE, keys,
};
pub use clipboard::{ClipboardManager, Selection};
/// What colour management names transfer functions and primaries by.
pub use smithay_client_toolkit::reexports::protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    Primaries, TransferFunction,
};
pub use gamma_control::GammaControl;
pub use headless::{ExitReport, HeadlessVaht};
pub use input_method::InputMethod;
pub use metrics::{BUILT_IN_METRICS, StyleMetrics};
pub use output_configurator::{
    ConfigurationResult, ConfiguredHead, HeadChange, OutputConfigurator,
};
pub use output_power::OutputPowerControl;
pub use pager::{PagedWorkspace, Pager};
pub use picture::{solid_png, solid_xcursor};
pub use sandbox::Sandbox;
pub use screenshot::{Bounds, Screenshot};
pub use toplevel_manager::{ManagedWindow, ToplevelManager};
pub use wait::{monotonic_now, wait_for_file};
pub use window_capture::WindowCapturer;
pub use window_list::{ListedWindow, WindowList};
pub use x11_client::X11Client;
