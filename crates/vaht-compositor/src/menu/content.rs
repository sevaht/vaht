//! One open menu as vaht draws it: its panel and where its lines go.
//!
//! [`MenuContent`] is a menu's panel content: the title, then a row per
//! item, laid out by vaht-ui with the style's `menu.*` elements, the
//! highlighted item `hovered`, an inert one `disabled`, a submenu's bullet
//! and a chosen toggle's check on the style's bullet side. [`MenuGeometry`] says where the
//! menu's outline, the line under its title and its separators go, as plain
//! colored rectangles around and over the panel, with arcs at rounded
//! corners, as a window frame's lines are.

use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::style::MenuMetrics;
use vaht_ui::{
    Corner, CornerRadii, Glyph, IconEmphasis, PanelContent, SizePolicy, Style, StyleElement,
    Typesetter, Widget, WidgetIcon, WidgetState,
};

use super::model::{MenuItem, MenuModel};
use crate::scene::Outline;

/// What a menu's panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuContent {
    /// The menu.
    model: MenuModel,
    /// The highlighted item, by index.
    hovered: Option<usize>,
    /// How the style sizes menus.
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

    /// Returns how wide and tall the panel is with `style`'s fonts: as wide
    /// as its widest text needs, and as tall as its title and rows.
    pub(crate) fn size(&self, style: &Style, typesetter: &mut Typesetter) -> vaht_ui::Size {
        let mut fit = |element, state, text: &str| {
            let style = style.element_style(element, state);
            typesetter.text_width(text, &style.text.font) + style.padding.horizontal()
        };
        let title = self.model.title.as_deref().map_or(0, |title| {
            fit(StyleElement::MenuTitle, WidgetState::Focused, title)
        });
        let icon_span = WidgetIcon::span(self.metrics.item_height);
        let items = self
            .model
            .items
            .iter()
            .filter_map(|item| Some((item.label()?, item.has_icon())))
            .map(|(label, has_icon)| {
                // A highlighted item may be drawn in a wider font.
                let text = fit(StyleElement::MenuItem, WidgetState::Focused, label).max(fit(
                    StyleElement::MenuItem,
                    WidgetState::Hovered,
                    label,
                ));
                if has_icon { text + icon_span } else { text }
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
                Widget::text(StyleElement::MenuTitle, title.clone())
                    .with_size(SizePolicy::Fixed(self.metrics.title_height)),
            );
            // The line under the title is drawn over this, as an outline.
            children.push(
                Widget::fill(StyleElement::MenuItem)
                    .with_size(SizePolicy::Fixed(self.metrics.border_width)),
            );
        }
        let row = SizePolicy::Fixed(self.metrics.item_height);
        for (index, item) in self.model.items.iter().enumerate() {
            let widget = match item {
                // The menu's own background shows through: a row of the frame's
                // texture would have its bevel round it, a box in the menu.
                MenuItem::Separator => Widget::fill(StyleElement::MenuItem).with_size(row),
                MenuItem::Inert { label } => Widget::text(StyleElement::MenuItem, label.clone())
                    .with_size(row)
                    .with_state(WidgetState::Disabled),
                MenuItem::Choice {
                    label, mark, icon, ..
                } => {
                    let widget = Self::with_icon(
                        Widget::text(StyleElement::MenuItem, label.clone())
                            .with_size(row)
                            .with_hit(index),
                        icon.as_ref(),
                    );
                    if *mark == Some(true) {
                        widget.with_mark_on(Glyph::Check, self.metrics.bullet_side)
                    } else {
                        widget
                    }
                }
                MenuItem::Submenu { label, icon, .. } => {
                    let widget = Self::with_icon(
                        Widget::text(StyleElement::MenuItem, label.clone())
                            .with_size(row)
                            .with_hit(index),
                        icon.as_ref(),
                    );
                    let side = self.metrics.bullet_side;
                    // With no bullet, the mark is one only a style's image shows.
                    let bullet = self.metrics.bullet.glyph(side).unwrap_or(Glyph::Submenu);
                    widget.with_mark_on(bullet, side)
                }
            };
            children.push(if self.hovered == Some(index) {
                widget.with_state(WidgetState::Hovered)
            } else {
                widget
            });
        }
        Widget::column(StyleElement::MenuFrame, children)
    }

    fn state(&self) -> WidgetState {
        WidgetState::Focused
    }
}

impl MenuContent {
    /// Returns `widget` showing `icon` at its left, if there is one.
    fn with_icon(widget: Widget<usize>, icon: Option<&vaht_ui::Icon>) -> Widget<usize> {
        match icon {
            Some(icon) => widget.with_icon(WidgetIcon {
                icon: icon.clone(),
                emphasis: IconEmphasis::Full,
            }),
            None => widget,
        }
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
            lines: {
                let [top, bottom, left, right] = Outline::lines_around(outer, border, corners);
                [top, bottom, left, right, title_line]
            },
            separators,
            line_width: metrics.border_width,
            corners,
        }
    }

    /// Returns where the corner square of `corner` is: as wide as its
    /// radius, in that corner of the menu; empty for a square corner.
    pub(crate) fn corner_square(&self, corner: Corner) -> Rectangle<i32, Logical> {
        Outline::corner_square(self.outer, corner, self.corners)
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
