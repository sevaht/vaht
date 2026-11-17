//! End-to-end tests: each module drives a headless vaht, its clients and
//! its command line, as a user's programs would.
//!
//! They are one test binary, not one per file, since each links all of
//! vaht and Smithay; nextest still runs every test in a process of its own.

mod bindings;
mod blur;
mod clipboard;
mod command_line;
mod cursor;
mod dialogs;
mod headless;
mod idle;
mod input_method;
mod ipc;
mod layers;
mod lock;
mod menus;
mod outputs;
mod pointer;
mod presentation;
mod rules;
mod sandbox;
mod settings;
mod taskbar;
mod themes;
mod ui;
mod wallpaper;
mod window_list;
mod window_management;
mod xwayland;
