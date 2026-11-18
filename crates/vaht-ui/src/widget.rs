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
    /// Whether it paints its element's background: not when it sits on its
    /// parent's, so a gradient runs unbroken behind several widgets rather
    /// than starting again in each.
    pub has_background: bool,
    /// The icon shown left of its text, if any.
    pub icon: Option<WidgetIcon>,
    /// The mark shown at one end, in a square as tall as it: a submenu's
    /// bullet, a chosen item's check.
    pub mark: Option<Glyph>,
    /// Which end the mark is at.
    pub mark_side: MarkSide,
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

    /// Returns a line of text being edited, with its cursor, that takes the
    /// room left over.
    #[must_use]
    pub const fn field(element: ThemeElement, field: TextField) -> Self {
        Self::new(
            element,
            WidgetKind::Field(field),
            SizePolicy::Fill { weight: 1 },
        )
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

    /// Returns the widget showing `mark` at its right end.
    #[must_use]
    pub const fn with_mark(self, mark: Glyph) -> Self {
        self.with_mark_on(mark, MarkSide::Right)
    }

    /// Returns the widget showing `mark` at the end `side` says. A check
    /// is drawn in the style's glyph color, other marks in its text color.
    #[must_use]
    pub const fn with_mark_on(mut self, mark: Glyph, side: MarkSide) -> Self {
        self.mark = Some(mark);
        self.mark_side = side;
        self
    }

    /// Returns the widget without a background of its own: what it shows
    /// (its text, icon or children) goes on what is behind it. A container's
    /// children still paint theirs.
    #[must_use]
    pub const fn without_background(mut self) -> Self {
        self.has_background = false;
        self
    }

    /// Returns the widget painting its element's background again, as
    /// every widget does until told otherwise.
    #[must_use]
    pub const fn with_background(mut self) -> Self {
        self.has_background = true;
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
            has_background: true,
            icon: None,
            mark: None,
            mark_side: MarkSide::Right,
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
    /// A line of text being edited.
    Field(TextField),
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

/// Which end of a widget its mark is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MarkSide {
    /// The left end.
    Left,
    /// The right end.
    #[default]
    Right,
}

/// A button picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// An X: close the window.
    Close,
    /// A box: maximize the window.
    Maximize,
    /// Two boxes, one behind the other: the window is maximized, and this
    /// puts it back.
    Restore,
    /// A low bar: minimize the window.
    Minimize,
    /// A high bar: shade the window to its titlebar.
    Shade,
    /// A dot: keep the window on every workspace.
    Stick,
    /// A triangle pointing left: the previous one.
    ArrowLeft,
    /// A triangle pointing right: the next one, or a submenu.
    ArrowRight,
    /// A filled square: a chosen menu item.
    Check,
    /// A menu's submenu bullet: a triangle a third of its square, pointing
    /// right (Fluxbox's `triangle`, on the right).
    BulletRight,
    /// The same, pointing left (on the left).
    BulletLeft,
    /// A square's outline, half its square (Fluxbox's `square`).
    BulletSquare,
    /// A small diamond (Fluxbox's `diamond`).
    BulletDiamond,
}

/// A line of text being edited, and where its cursor is.
///
/// It is drawn left-justified, scrolled so the cursor stays in view, with
/// the cursor a line in the text's color.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextField {
    /// The text.
    text: String,
    /// The cursor, as a byte offset into the text, at a character boundary.
    cursor: usize,
}

impl TextField {
    /// Returns `text` with the cursor at byte `cursor`, moved back to the
    /// character boundary at or before it.
    #[must_use]
    pub fn new(text: impl Into<String>, cursor: usize) -> Self {
        let text = text.into();
        let mut cursor = cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        Self { text, cursor }
    }

    /// Returns the text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the cursor, as a byte offset into the text.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Returns the text before the cursor.
    #[must_use]
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_cursor_stays_on_a_character_boundary() {
        let field = TextField::new("aé", 2);
        assert_eq!(field.cursor(), 1, "inside é, it moves back before it");
        assert_eq!(
            TextField::new("ab", 9).cursor(),
            2,
            "past the end, at the end"
        );
        assert_eq!(field.before_cursor(), "a");
    }
}
