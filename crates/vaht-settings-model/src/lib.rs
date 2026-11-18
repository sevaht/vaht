//! What the settings window shows and decides, without drawing anything.
//!
//! vaht describes its settings as JSON Schema (`vaht config schema`) and
//! says what they are now (`vaht config get`). This crate turns those
//! into a form to fill in ([`Form`]: sections of [`Setting`]s, each with the
//! kind of value it takes), and keeps track of what the user has done with
//! it ([`SettingsEditor`]): which settings differ from their defaults, which
//! are changed and not yet written, which match a search, and the edits to
//! send for a change.
//!
//! Nothing here talks to vaht or draws: the window feeds it what vaht
//! said and sends what it asks to be sent, so every decision is a function
//! of plain values and is tested without a display.

mod editor;
mod form;
mod label;

pub use editor::SettingsEditor;
pub use form::{Alternative, Choice, Form, FormError, Section, Setting, SettingKind, Shape};
