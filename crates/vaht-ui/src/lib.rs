//! vaht's panel toolkit: widgets, layout, themes and drawing.
//!
//! Every surface vaht draws itself (window frames, the taskbar, menus,
//! tooltips) is content described as `Widget`s and shown by one `Panel` type
//! that lays it out, paints it with tiny-skia and cosmic-text, and hit-tests
//! it. This crate knows nothing about Wayland, so panels can be rendered and
//! tested on their own. See `docs/design/01-object-model.md` §6.
//!
//! Nothing is implemented yet.
