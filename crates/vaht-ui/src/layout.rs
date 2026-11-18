//! Where each widget of a tree goes, and which one is under a point.
//!
//! A [`Layout`] flattens a widget tree into [`Placed`] rectangles in drawing
//! order: a parent before its children, so children are drawn over their
//! parent's background. Hit-testing walks the same list backwards, so the
//! widget drawn on top is the one that is clicked.

use crate::geometry::{Insets, Point, Rect};
use crate::icon::WidgetIcon;
use crate::style::{StyleElement, WidgetState};
use crate::widget::{Glyph, MarkSide, SizePolicy, TextField, Widget, WidgetKind};

/// What a placed widget draws inside its rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A line of text.
    Text(String),
    /// A button picture.
    Glyph(Glyph),
    /// A line of text being edited.
    Field(TextField),
    /// Only the element's background (fills, rows and columns).
    Background,
}

/// One widget with its rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed<Hit> {
    /// Where it is, in the panel's pixels.
    pub rect: Rect,
    /// What styles it.
    pub element: StyleElement,
    /// What it draws.
    pub content: Content,
    /// What clicking it reports.
    pub hit: Option<Hit>,
    /// Whether it is drawn pressed while held.
    pub is_button: bool,
    /// The state it is drawn in, when not its panel's.
    pub state: Option<WidgetState>,
    /// Whether it paints its element's background.
    pub has_background: bool,
    /// The icon shown left of its text, if any.
    pub icon: Option<WidgetIcon>,
    /// The mark shown at one end, if any.
    pub mark: Option<Glyph>,
    /// Which end.
    pub mark_side: MarkSide,
}

/// The room a row or column keeps around and between its children, as its
/// element's style says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Space {
    /// Kept free inside its edges.
    pub inset: Insets,
    /// Kept free between one child and the next, in pixels.
    pub gap: u32,
}

/// A widget tree placed in an area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout<Hit> {
    /// Every widget, parents before children.
    placed: Vec<Placed<Hit>>,
}

impl<Hit: Copy> Layout<Hit> {
    /// Places `root` in `area`; `fit_width` returns the room a text widget of
    /// an element needs, padding included.
    pub fn new(
        root: &Widget<Hit>,
        area: Rect,
        fit_width: &mut dyn FnMut(&str, StyleElement) -> u32,
    ) -> Self {
        Self::with_space(root, area, fit_width, &mut |_| Space::default())
    }

    /// Places `root` in `area` as [`Layout::new`] does, with each row and
    /// column keeping the room `space` gives for its element.
    pub fn with_space(
        root: &Widget<Hit>,
        area: Rect,
        fit_width: &mut dyn FnMut(&str, StyleElement) -> u32,
        space: &mut dyn FnMut(StyleElement) -> Space,
    ) -> Self {
        let mut placer = Placer {
            fit_width,
            space,
            placed: Vec::new(),
        };
        placer.place(root, area);
        Self {
            placed: placer.placed,
        }
    }

    /// Returns every widget in drawing order.
    #[must_use]
    pub fn placed(&self) -> &[Placed<Hit>] {
        &self.placed
    }

    /// Returns what clicking at `point` reports: the topmost clickable
    /// widget there.
    #[must_use]
    pub fn hit_at(&self, point: Point) -> Option<Hit> {
        self.placed
            .iter()
            .rev()
            .find(|placed| placed.hit.is_some() && placed.rect.contains(point))
            .and_then(|placed| placed.hit)
    }
}

/// Which way a container lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// Left to right.
    Horizontal,
    /// Top to bottom.
    Vertical,
}

impl Axis {
    /// Returns `rect`'s extent along this axis.
    const fn length(self, rect: Rect) -> u32 {
        match self {
            Self::Horizontal => rect.width,
            Self::Vertical => rect.height,
        }
    }

    /// Returns `rect`'s extent across this axis.
    const fn thickness(self, rect: Rect) -> u32 {
        match self {
            Self::Horizontal => rect.height,
            Self::Vertical => rect.width,
        }
    }

    /// Returns the part of `area` starting `start` pixels along this axis
    /// and `length` pixels long.
    fn slice(self, area: Rect, start: u32, length: u32) -> Rect {
        let offset = i32::try_from(start).expect("panel coordinates fit in i32");
        match self {
            Self::Horizontal => Rect::new(area.x + offset, area.y, length, area.height),
            Self::Vertical => Rect::new(area.x, area.y + offset, area.width, length),
        }
    }
}

/// Places a widget tree: each widget's area, parents before children.
struct Placer<'fit, Hit> {
    /// Returns the room a text widget of an element needs, padding
    /// included.
    fit_width: &'fit mut dyn FnMut(&str, StyleElement) -> u32,
    /// Returns the room a row or column of an element keeps.
    space: &'fit mut dyn FnMut(StyleElement) -> Space,
    /// Every widget placed so far.
    placed: Vec<Placed<Hit>>,
}

impl<Hit: Copy> Placer<'_, Hit> {
    /// Places `widget` in `area`, then its descendants.
    fn place(&mut self, widget: &Widget<Hit>, area: Rect) {
        let content = match &widget.kind {
            WidgetKind::Text(text) => Content::Text(text.clone()),
            WidgetKind::Glyph(glyph) => Content::Glyph(*glyph),
            WidgetKind::Field(field) => Content::Field(field.clone()),
            WidgetKind::Fill | WidgetKind::Row(_) | WidgetKind::Column(_) => Content::Background,
        };
        self.placed.push(Placed {
            rect: area,
            element: widget.element,
            content,
            hit: widget.hit,
            is_button: widget.is_button,
            state: widget.state,
            has_background: widget.has_background,
            icon: widget.icon.clone(),
            mark: widget.mark,
            mark_side: widget.mark_side,
        });
        let (axis, children) = match &widget.kind {
            WidgetKind::Row(children) => (Axis::Horizontal, children),
            WidgetKind::Column(children) => (Axis::Vertical, children),
            WidgetKind::Text(_)
            | WidgetKind::Glyph(_)
            | WidgetKind::Field(_)
            | WidgetKind::Fill => return,
        };
        let space = (self.space)(widget.element);
        let inner = area.inset(space.inset);
        // The gaps come out of what the children share.
        let gaps = space.gap * u32::try_from(children.len().saturating_sub(1)).unwrap_or(0);
        let shared = axis.slice(inner, 0, axis.length(inner).saturating_sub(gaps));
        let lengths = self.distribute(axis, shared, children);
        let mut start = 0;
        for (child, length) in children.iter().zip(lengths) {
            self.place(child, axis.slice(inner, start, length));
            start += length + space.gap;
        }
    }

    /// Divides `area`'s main axis among `children`: fixed, fitted and
    /// square children first, in order, then equal shares (each up to its
    /// most) for the sharing ones, then what remains among the fills by
    /// weight. When there is not enough room, later children get less.
    fn distribute(&mut self, axis: Axis, area: Rect, children: &[Widget<Hit>]) -> Vec<u32> {
        let available = axis.length(area);
        let mut remaining = available;
        let mut lengths: Vec<u32> = children
            .iter()
            .map(|child| {
                let wanted = match child.size {
                    SizePolicy::Fixed(length) => length,
                    SizePolicy::Square => axis.thickness(area),
                    SizePolicy::Fit => self.natural_length(child, axis, axis.thickness(area)),
                    SizePolicy::Share { .. } | SizePolicy::Fill { .. } => 0,
                };
                let granted = wanted.min(remaining);
                remaining -= granted;
                granted
            })
            .collect();
        let sharing = children
            .iter()
            .filter(|child| matches!(child.size, SizePolicy::Share { .. }))
            .count();
        if let Some(each) = u32::try_from(sharing)
            .ok()
            .filter(|count| *count > 0)
            .map(|count| remaining / count)
        {
            for (child, length) in children.iter().zip(&mut lengths) {
                if let SizePolicy::Share { max } = child.size {
                    *length = each.min(max);
                    remaining -= *length;
                }
            }
        }
        let total_weight: u32 = children
            .iter()
            .map(|child| match child.size {
                SizePolicy::Fill { weight } => weight,
                SizePolicy::Fixed(_)
                | SizePolicy::Fit
                | SizePolicy::Square
                | SizePolicy::Share { .. } => 0,
            })
            .sum();
        if total_weight > 0 {
            let free = remaining;
            let mut given = 0;
            let mut weight_so_far = 0;
            for (child, length) in children.iter().zip(&mut lengths) {
                if let SizePolicy::Fill { weight } = child.size {
                    // Shares end at cumulative boundaries, so rounding never
                    // loses a pixel: the fills together get exactly what was
                    // free.
                    weight_so_far += weight;
                    let end = u32::try_from(
                        u64::from(free) * u64::from(weight_so_far) / u64::from(total_weight),
                    )
                    .expect("a share of a u32 fits in a u32");
                    *length = end - given;
                    given = end;
                }
            }
        }
        lengths
    }

    /// Returns how much room `widget` needs along `axis` when fitted, in a
    /// container `thickness` across.
    fn natural_length(&mut self, widget: &Widget<Hit>, axis: Axis, thickness: u32) -> u32 {
        match &widget.kind {
            WidgetKind::Text(text) => match axis {
                Axis::Horizontal => {
                    let icon = widget
                        .icon
                        .as_ref()
                        .map_or(0, |_| WidgetIcon::span(thickness));
                    (self.fit_width)(text, widget.element) + icon
                }
                // A text line's height is its container's; columns of text
                // give each line a fixed size instead.
                Axis::Vertical => 0,
            },
            WidgetKind::Row(children) | WidgetKind::Column(children) => {
                let space = (self.space)(widget.element);
                let own = match axis {
                    Axis::Horizontal => space.inset.horizontal(),
                    Axis::Vertical => space.inset.vertical(),
                };
                let gaps = space.gap * u32::try_from(children.len().saturating_sub(1)).unwrap_or(0);
                own + gaps
                    + children
                        .iter()
                        .map(|child| match child.size {
                            SizePolicy::Fixed(length) => length,
                            SizePolicy::Fit => self.natural_length(child, axis, thickness),
                            SizePolicy::Square
                            | SizePolicy::Share { .. }
                            | SizePolicy::Fill { .. } => 0,
                        })
                        .sum::<u32>()
            }
            WidgetKind::Glyph(_) | WidgetKind::Field(_) | WidgetKind::Fill => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text measurer that makes every character 10 pixels wide.
    fn ten_per_character(text: &str, _: StyleElement) -> u32 {
        u32::try_from(text.chars().count() * 10).expect("small")
    }

    /// Lays out `root` in a 200 by 20 area.
    fn lay_out(root: &Widget<u8>) -> Layout<u8> {
        Layout::new(root, Rect::new(0, 0, 200, 20), &mut ten_per_character)
    }

    /// Returns the rectangles of the children of the root, in order.
    fn child_rects(layout: &Layout<u8>) -> Vec<Rect> {
        layout
            .placed()
            .iter()
            .skip(1)
            .map(|placed| placed.rect)
            .collect()
    }

    #[test]
    fn a_titlebar_row_gives_buttons_squares_and_the_label_the_rest() {
        let row = Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::fill(StyleElement::Label),
                Widget::glyph(StyleElement::Button, Glyph::Minimize),
                Widget::glyph(StyleElement::Button, Glyph::Close),
            ],
        );
        assert_eq!(
            child_rects(&lay_out(&row)),
            [
                Rect::new(0, 0, 160, 20),
                Rect::new(160, 0, 20, 20),
                Rect::new(180, 0, 20, 20)
            ]
        );
    }

    #[test]
    fn fills_share_the_remainder_by_weight_without_losing_pixels() {
        let row = Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::text(StyleElement::Label, "abcd"),
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Fill { weight: 1 }),
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Fill { weight: 2 }),
            ],
        );
        let rects = child_rects(&lay_out(&row));
        assert_eq!(rects[0].width, 40, "text fits its four characters");
        assert_eq!(
            rects[1].width + rects[2].width,
            160,
            "fills take exactly the rest"
        );
        assert_eq!(rects[1].width, 53);
        assert_eq!(rects[2].x, 93);
    }

    #[test]
    fn when_room_runs_out_later_children_get_less() {
        let row = Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Fixed(150)),
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Fixed(100)),
            ],
        );
        let widths: Vec<u32> = child_rects(&lay_out(&row))
            .iter()
            .map(|rect| rect.width)
            .collect();
        assert_eq!(widths, [150, 50]);
    }

    #[test]
    fn hit_testing_finds_the_topmost_clickable_widget() {
        let row = Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::fill(StyleElement::Label).with_hit(1),
                Widget::glyph(StyleElement::Button, Glyph::Close).with_hit(2),
            ],
        )
        .with_hit(0);
        let layout = lay_out(&row);
        assert_eq!(layout.hit_at(Point::new(10, 10)), Some(1));
        assert_eq!(layout.hit_at(Point::new(190, 10)), Some(2));
        assert_eq!(
            layout.hit_at(Point::new(250, 10)),
            None,
            "outside the panel"
        );
    }

    #[test]
    fn sharing_children_split_the_room_equally_up_to_their_most() {
        let entry = |max| Widget::fill(StyleElement::Label).with_size(SizePolicy::Share { max });
        let widths = |width, count| -> Vec<u32> {
            let root: Widget<()> = Widget::row(StyleElement::Titlebar, vec![entry(128); count]);
            Layout::new(&root, Rect::new(0, 0, width, 20), &mut |_, _| 0)
                .placed()
                .iter()
                .skip(1)
                .map(|placed| placed.rect.width)
                .collect()
        };
        assert_eq!(
            widths(1000, 3),
            [128, 128, 128],
            "room to spare: each at its most"
        );
        assert_eq!(widths(300, 3), [100, 100, 100], "crowded: equal shares");
    }

    #[test]
    fn a_rows_space_insets_its_parts_and_goes_between_them() {
        let row = Widget::row(
            StyleElement::Titlebar,
            vec![
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Square),
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Fill { weight: 1 }),
                Widget::fill(StyleElement::Label).with_size(SizePolicy::Square),
            ],
        );
        let space = Space {
            inset: Insets {
                top: 2,
                right: 3,
                bottom: 2,
                left: 3,
            },
            gap: 4,
        };
        let layout = Layout::with_space(
            &row,
            Rect::new(0, 0, 100, 20),
            &mut ten_per_character,
            &mut |_| space,
        );
        // 16 high inside, 94 wide: buttons 16 square, 4 apart from the label,
        // which has the 94 - 8 - 32 left.
        assert_eq!(
            child_rects(&layout),
            [
                Rect::new(3, 2, 16, 16),
                Rect::new(23, 2, 54, 16),
                Rect::new(81, 2, 16, 16)
            ]
        );
    }
}
