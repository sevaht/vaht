//! The vaht settings window: a client of a running vaht that edits its
//! configuration.
//!
//! The window holds no configuration logic. vaht describes its settings
//! (`vaht config schema`), says what they are now, checks every change
//! and runs it at once; this crate draws what [`vaht_settings_model`] decides
//! and passes the user's changes on. [`link`] is the line to vaht, [`app`]
//! the window, [`controls`] the widget for each kind of setting.

pub mod app;
pub mod controls;
pub mod dialog;
pub mod fonts;
pub mod link;
