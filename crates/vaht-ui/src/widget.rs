//! What a panel shows, described as a tree of widgets.
//!
//! A panel's content builds a [`Widget`] tree from its own state; nothing in
//! the tree draws itself or handles input. Each widget names the
//! [`ThemeElement`] that styles it, how much room it wants
//! ([`SizePolicy`]), and optionally the `Hit` value the content wants back
//! when the widget is clicked. A *button* also looks pressed while held; a
//! plain hit region (a title to drag, say) never changes look.

use crate::icon::WidgetIcon;
use crate::theme::{ThemeElement, WidgetState};

/// One node of a panel's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widget<Hit> {
    /// What it shows.
    pub kind: WidgetKind<Hit>,
    /// What styles it.
    pub element: ThemeElement,
    /// How much room it takes along its parent's main axis.
    pub size: SizePolicy,
    /// What clicking it reports, if it can be clicked.
    pub hit: Option<Hit>,
    /// Whether it is a button: drawn pressed while the pointer holds it.
    pub is_button: bool,
    /// The state it is drawn in, when not its panel's: a focused window's
    /// entry among unfocused ones.
    pub state: Option<WidgetState>,
    /// The icon shown left of its text, if any.
    pub icon: Option<WidgetIcon>,
}

impl<Hit> Widget<Hit> {
    /// Returns a text widget that takes the room its text needs.
    #[must_use]
    pub fn text(element: ThemeElement, text: impl Into<String>) -> Self {
        Self::new(element, WidgetKind::Text(text.into()), SizePolicy::Fit)
    }

    /// Returns a square widget showing `glyph`.
    #[must_use]
    pub const fn glyph(element: ThemeElement, glyph: Glyph) -> Self {
        Self::new(element, WidgetKind::Glyph(glyph), SizePolicy::Square)
    }

    /// Returns a widget that only shows its element's background and takes
    /// the room left over.
    #[must_use]
    pub const fn fill(element: ThemeElement) -> Self {
        Self::new(element, WidgetKind::Fill, SizePolicy::Fill { weight: 1 })
    }

    /// Returns a horizontal row of `children` on `element`'s background.
    #[must_use]
    pub const fn row(element: ThemeElement, children: Vec<Self>) -> Self {
        Self::new(
            element,
            WidgetKind::Row(children),
            SizePolicy::Fill { weight: 1 },
        )
    }

    /// Returns a vertical column of `children` on `element`'s background.
    #[must_use]
    pub const fn column(element: ThemeElement, children: Vec<Self>) -> Self {
        Self::new(
            element,
            WidgetKind::Column(children),
            SizePolicy::Fill { weight: 1 },
        )
    }

    /// Returns the widget taking room according to `size`.
    #[must_use]
    pub fn with_size(mut self, size: SizePolicy) -> Self {
        self.size = size;
        self
    }

    /// Returns the widget reporting `hit` when clicked, without changing its
    /// look while held: a region to drag or double-click, not a button.
    #[must_use]
    pub fn with_hit(mut self, hit: Hit) -> Self {
        self.hit = Some(hit);
        self.is_button = false;
        self
    }

    /// Returns the widget showing `icon` left of its text.
    #[must_use]
    pub fn with_icon(mut self, icon: WidgetIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Returns the widget drawn in `state` rather than its panel's.
    #[must_use]
    pub const fn with_state(mut self, state: WidgetState) -> Self {
        self.state = Some(state);
        self
    }

    /// Returns the widget as a button: it reports `hit` when clicked and is
    /// drawn pressed while held.
    #[must_use]
    pub fn with_button(mut self, hit: Hit) -> Self {
        self.hit = Some(hit);
        self.is_button = true;
        self
    }

    /// Creates a widget that cannot be clicked.
    const fn new(element: ThemeElement, kind: WidgetKind<Hit>, size: SizePolicy) -> Self {
        Self {
            kind,
            element,
            size,
            hit: None,
            is_button: false,
            state: None,
            icon: None,
        }
    }
}

/// What a widget shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind<Hit> {
    /// A line of text.
    Text(String),
    /// A button picture.
    Glyph(Glyph),
    /// Only the element's background.
    Fill,
    /// Children side by side, left to right.
    Row(Vec<Widget<Hit>>),
    /// Children stacked, top to bottom.
    Column(Vec<Widget<Hit>>),
}

/// How much room a widget takes along its parent's main axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SizePolicy {
    /// Exactly this many pixels.
    Fixed(u32),
    /// As much as its content needs (text plus padding).
    Fit,
    /// As long as the parent's cross axis: a square in a row.
    Square,
    /// An equal share, with the other sharing siblings, of the room the
    /// fixed, fitted and square ones leave, but no more than `max` pixels:
    /// a window list's entries.
    Share {
        /// The most room it takes.
        max: u32,
    },
    /// A share of the room the other children leave, by `weight`.
    Fill {
        /// This child's share relative to its fill siblings'.
        weight: u32,
    },
}

/// A button picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// An X: close the window.
    Close,
    /// A box: maximize the window.
    Maximize,
    /// A low bar: minimize the window.
    Minimize,
    /// A high bar: shade the window to its titlebar.
    Shade,
    /// A dot: keep the window on every workspace.
    Stick,
    /// A triangle pointing left: the previous one.
    ArrowLeft,
    /// A triangle pointing right: the next one.
    ArrowRight,
}
