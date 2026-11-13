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
mod headless;
mod screenshot;

pub use client::{
    BUTTON_LEFT, BUTTON_RIGHT, KeyboardFocus, TestClient, TestLayer, WINDOW_COLOR, WINDOW_SIZE,
    keys,
};
pub use headless::{ExitReport, HeadlessVaht};
pub use screenshot::{Bounds, Screenshot};
