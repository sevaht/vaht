//! vaht's panel toolkit: widgets, layout, styles and drawing.
//!
//! Every surface vaht draws itself (window frames, the taskbar, menus,
//! tooltips) is a [`Panel`]: its [`PanelContent`] describes what it shows as
//! a tree of [`Widget`]s, and the panel lays them out, paints them with a
//! [`Style`] and a [`Typesetter`], and hit-tests them. Content types never
//! draw or hit-test anything themselves, which is what keeps vaht to one
//! drawing path (01 §6).
//!
//! The crate knows nothing about Wayland: panels paint into plain pixel
//! buffers, so they can be rendered and tested on their own.

mod canvas;
pub mod color;
pub mod corner;
pub mod geometry;
pub mod icon;
pub mod image;
pub mod layout;
pub mod length;
pub mod panel;
pub mod point_size;
pub mod raster;
pub mod style;
pub mod text;
pub mod texture;
pub mod wallpaper;
pub mod widget;

#[doc(inline)]
pub use color::Color;
#[doc(inline)]
pub use corner::{Corner, CornerArc, CornerRadii};
#[doc(inline)]
pub use geometry::{Insets, Point, Rect, Size};
#[doc(inline)]
pub use icon::{Icon, IconEmphasis, IconStore, WidgetIcon};
#[doc(inline)]
pub use image::{Image, ImageError};
#[doc(inline)]
pub use panel::{Panel, PanelContent};
#[doc(inline)]
pub use style::{Style, StyleElement, WidgetState};
#[doc(inline)]
pub use text::{FontWeight, Typesetter, system_font};
/// The pixels panels and corner arcs are painted into.
pub use tiny_skia::Pixmap;
#[doc(inline)]
pub use wallpaper::{Wallpaper, WallpaperError, WallpaperFit};
#[doc(inline)]
pub use widget::{Glyph, MarkSide, SizePolicy, Widget};
