//! One open menu as vaht draws it: its panel and where its lines go.
//!
//! [`MenuContent`] is a menu's panel content: the title, then a row per
//! item, laid out by vaht-ui with the theme's `menu.*` elements, the
//! highlighted item `hovered`, an inert one `disabled`, a submenu's arrow
//! and a chosen toggle's check at the right. [`MenuGeometry`] says where the
//! menu's outline, the line under its title and its separators go, as plain
//! colored rectangles around and over the panel, with arcs at rounded
//! corners, as a window frame's lines are.

use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::theme::MenuMetrics;
use vaht_ui::{
    Corner, CornerRadii, Glyph, PanelContent, SizePolicy, Theme, ThemeElement, Typesetter, Widget,
    WidgetState,
};

use super::model::{MenuItem, MenuModel};

/// What a menu's panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuContent {
    /// The menu.
    model: MenuModel,
    /// The highlighted item, by index.
    hovered: Option<usize>,
    /// How the theme sizes menus.
    metrics: MenuMetrics,
}

impl MenuContent {
    /// The content of `model`, sized by `metrics`, nothing highlighted.
    pub(crate) const fn new(model: MenuModel, metrics: MenuMetrics) -> Self {
        Self {
            model,
            hovered: None,
            metrics,
        }
    }

    /// Returns the menu.
    pub(crate) const fn model(&self) -> &MenuModel {
        &self.model
    }

    /// Returns the highlighted item, by index.
    pub(crate) const fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// Returns how wide and tall the panel is with `theme`'s fonts: as wide
    /// as its widest text needs, and as tall as its title and rows.
    pub(crate) fn size(&self, theme: &Theme, typesetter: &mut Typesetter) -> vaht_ui::Size {
        let mut fit = |element, state, text: &str| {
            let style = theme.style(element, state);
            typesetter.text_width(text, &style.text.font) + style.padding.horizontal()
        };
        let title = self.model.title.as_deref().map_or(0, |title| {
            fit(ThemeElement::MenuTitle, WidgetState::Focused, title)
        });
        let items = self
            .model
            .items
            .iter()
            .filter_map(MenuItem::label)
            .map(|label| {
                // A highlighted item may be drawn in a wider font.
                fit(ThemeElement::MenuItem, WidgetState::Focused, label).max(fit(
                    ThemeElement::MenuItem,
                    WidgetState::Hovered,
                    label,
                ))
            })
            .max()
            .unwrap_or(0);
        vaht_ui::Size::new(title.max(items).max(1), self.height())
    }

    /// Returns where item `index`'s row starts, down from the panel's top.
    pub(crate) fn row_top(&self, index: usize) -> u32 {
        self.title_span()
            + u32::try_from(index)
                .unwrap_or(u32::MAX)
                .saturating_mul(self.metrics.item_height)
    }

    /// Returns the panel's height: its title and its rows.
    fn height(&self) -> u32 {
        self.row_top(self.model.items.len())
    }

    /// Returns how much the title takes, with the line under it.
    const fn title_span(&self) -> u32 {
        if self.model.title.is_some() {
            self.metrics.title_height + self.metrics.border_width
        } else {
            0
        }
    }

    /// Highlights item `index`, or nothing; returns whether that changed.
    pub(crate) fn set_hovered(&mut self, index: Option<usize>) -> bool {
        let index = index.filter(|index| {
            self.model
                .items
                .get(*index)
                .is_some_and(MenuItem::is_selectable)
        });
        let changed = self.hovered != index;
        self.hovered = index;
        changed
    }
}

impl PanelContent for MenuContent {
    type Hit = usize;

    fn build(&self) -> Widget<usize> {
        let mut children = Vec::new();
        if let Some(title) = &self.model.title {
            children.push(
                Widget::text(ThemeElement::MenuTitle, title.clone())
                    .with_size(SizePolicy::Fixed(self.metrics.title_height)),
            );
            // The line under the title is drawn over this, as an outline.
            children.push(
                Widget::fill(ThemeElement::MenuFrame)
                    .with_size(SizePolicy::Fixed(self.metrics.border_width)),
            );
        }
        let row = SizePolicy::Fixed(self.metrics.item_height);
        for (index, item) in self.model.items.iter().enumerate() {
            let widget = match item {
                MenuItem::Separator => Widget::fill(ThemeElement::MenuFrame).with_size(row),
                MenuItem::Inert { label } => Widget::text(ThemeElement::MenuItem, label.clone())
                    .with_size(row)
                    .with_state(WidgetState::Disabled),
                MenuItem::Choice { label, mark, .. } => {
                    let widget = Widget::text(ThemeElement::MenuItem, label.clone())
                        .with_size(row)
                        .with_hit(index);
                    if *mark == Some(true) {
                        widget.with_mark(Glyph::Check)
                    } else {
                        widget
                    }
                }
                MenuItem::Submenu { label, .. } => {
                    Widget::text(ThemeElement::MenuItem, label.clone())
                        .with_size(row)
                        .with_hit(index)
                        .with_mark(Glyph::ArrowRight)
                }
            };
            children.push(if self.hovered == Some(index) {
                widget.with_state(WidgetState::Hovered)
            } else {
                widget
            });
        }
        Widget::column(ThemeElement::MenuFrame, children)
    }

    fn state(&self) -> WidgetState {
        WidgetState::Focused
    }
}

/// Where each part of an open menu goes, in global coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuGeometry {
    /// Everything the menu covers, its outline included.
    pub(crate) outer: Rectangle<i32, Logical>,
    /// The panel: the title and the items.
    pub(crate) panel: Rectangle<i32, Logical>,
    /// The outline's lines: top, bottom, left, right; then the line under
    /// the title, empty without one.
    pub(crate) lines: [Rectangle<i32, Logical>; 5],
    /// The separators' lines.
    pub(crate) separators: Vec<Rectangle<i32, Logical>>,
    /// How wide the lines are.
    pub(crate) line_width: u32,
    /// How round the menu's corners are.
    pub(crate) corners: CornerRadii,
}

impl MenuGeometry {
    /// Returns where the parts of a menu showing `content` in a panel of
    /// `size` go, its outline's top-left corner at `origin`.
    pub(crate) fn at(
        origin: Point<i32, Logical>,
        size: vaht_ui::Size,
        content: &MenuContent,
    ) -> Self {
        let metrics = content.metrics;
        let border = to_i32(metrics.border_width);
        let (width, height) = (to_i32(size.width), to_i32(size.height));
        let rectangle = |x, y, width, height| Rectangle::new((x, y).into(), (width, height).into());
        let outer = rectangle(origin.x, origin.y, width + 2 * border, height + 2 * border);
        let panel = rectangle(origin.x + border, origin.y + border, width, height);
        let most = to_u32((outer.size.w / 2).min(outer.size.h / 2));
        let corners = <Corner as strum::IntoEnumIterator>::iter()
            .fold(metrics.corners, |radii, corner| {
                radii.with(corner, metrics.corners.radius(corner).min(most))
            });
        let radius = |corner| to_i32(corners.radius(corner));
        let (top_left, top_right) = (radius(Corner::TopLeft), radius(Corner::TopRight));
        let (bottom_left, bottom_right) = (radius(Corner::BottomLeft), radius(Corner::BottomRight));
        let (right, bottom) = (outer.loc.x + outer.size.w, outer.loc.y + outer.size.h);
        let title_line = if content.model.title.is_some() {
            rectangle(
                panel.loc.x,
                panel.loc.y + to_i32(metrics.title_height),
                width,
                border,
            )
        } else {
            Rectangle::default()
        };
        // A separator is a line across the middle of its row, an item-high
        // square in from each end, as Fluxbox draws it.
        let item_height = to_i32(metrics.item_height);
        let separators = content
            .model
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, MenuItem::Separator))
            .map(|(index, _)| {
                rectangle(
                    panel.loc.x + item_height + 1,
                    panel.loc.y + to_i32(content.row_top(index)) + item_height / 2,
                    (width - 2 * item_height - 1).max(0),
                    1,
                )
            })
            .collect();
        Self {
            outer,
            panel,
            lines: [
                rectangle(
                    outer.loc.x + top_left,
                    outer.loc.y,
                    outer.size.w - top_left - top_right,
                    border,
                ),
                rectangle(
                    outer.loc.x + bottom_left,
                    bottom - border,
                    outer.size.w - bottom_left - bottom_right,
                    border,
                ),
                rectangle(
                    outer.loc.x,
                    outer.loc.y + top_left,
                    border,
                    outer.size.h - top_left - bottom_left,
                ),
                rectangle(
                    right - border,
                    outer.loc.y + top_right,
                    border,
                    outer.size.h - top_right - bottom_right,
                ),
                title_line,
            ],
            separators,
            line_width: metrics.border_width,
            corners,
        }
    }

    /// Returns where the corner square of `corner` is: as wide as its
    /// radius, in that corner of the menu; empty for a square corner.
    pub(crate) fn corner_square(&self, corner: Corner) -> Rectangle<i32, Logical> {
        let radius = to_i32(self.corners.radius(corner));
        let outer = self.outer;
        let x = if corner.is_left() {
            outer.loc.x
        } else {
            outer.loc.x + outer.size.w - radius
        };
        let y = if corner.is_top() {
            outer.loc.y
        } else {
            outer.loc.y + outer.size.h - radius
        };
        Rectangle::new((x, y).into(), (radius, radius).into())
    }

    /// Returns how round the panel's corners are: the menu's, just inside
    /// the outline.
    pub(crate) fn panel_corners(&self) -> CornerRadii {
        self.corners.inset(self.line_width)
    }

    /// Returns whether the menu covers global `point`, its cut corners
    /// aside.
    pub(crate) fn covers(&self, point: Point<f64, Logical>) -> bool {
        let pixel = point.to_i32_floor();
        let inside = pixel - self.outer.loc;
        let size = vaht_ui::Size::new(to_u32(self.outer.size.w), to_u32(self.outer.size.h));
        self.outer.contains(pixel)
            && !self
                .corners
                .cuts(size, vaht_ui::Point::new(inside.x, inside.y))
    }

    /// Returns global `point` in the panel's own pixels.
    pub(crate) fn panel_point(&self, point: Point<f64, Logical>) -> vaht_ui::Point {
        let inside = point.to_i32_floor() - self.panel.loc;
        vaht_ui::Point::new(inside.x, inside.y)
    }
}

/// Converts a pixel extent to a signed coordinate.
fn to_i32(extent: u32) -> i32 {
    i32::try_from(extent).expect("menu sizes are far below i32::MAX")
}

/// Converts a non-negative coordinate extent to a size.
fn to_u32(extent: i32) -> u32 {
    u32::try_from(extent).unwrap_or(0)
}
