//! What vaht draws around a window: a titlebar, a handle and border lines.
//!
//! From the top, as Fluxbox draws it: a border line, the titlebar, a line,
//! the content, a line, the handle (a strip with a grip at each end, when
//! the style gives it a height), and a last line; lines run down both
//! sides. The titlebar and the handle are [`Panel`]s; vaht-ui lays them
//! out, paints them and hit-tests them, and this module only says what they
//! show (01 §4.3). The lines are plain colored rectangles, which cost no
//! pixel buffers. The frame is never one big buffer around the window,
//! which would cost megabytes per window.
//!
//! A style can round the frame's corners (`window.border`'s
//! `corner_radius`). A top corner is rounded within the titlebar and a
//! bottom one within the handle, so the window's content is never cut, as
//! Fluxbox rounds bottom corners only when there is a handle. The lines
//! stop short of a rounded corner, a small arc joins them there, and the
//! panels' corners are cut away beneath it; the pointer passes through
//! what was cut.
//! Nothing here knows a renderer: the scene turns the pixels and the
//! geometry into render elements.
//!
//! Geometry is derived, never stored: [`FrameGeometry::around`] computes every
//! part from the window's content area and the style's metrics, so the frame
//! cannot drift from the window it surrounds.

use smithay::utils::{Logical, Point, Rectangle};
use vaht_ui::style::Metrics;
use vaht_ui::{
    Corner, CornerRadii, Glyph, Icon, IconEmphasis, Panel, PanelContent, SizePolicy, Style,
    StyleElement, Typesetter, Widget, WidgetIcon, WidgetState,
};

use super::WindowDrag;
use super::placement::Axes;
use super::resize::{HorizontalEdge, ResizeEdges, VerticalEdge};
use crate::config::WindowManagerAction;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};

/// What clicking a titlebar can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TitlebarHit {
    /// The title: dragging it will move the window.
    Label,
    /// A button.
    Button(TitlebarButton),
}

/// A titlebar button.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TitlebarButton {
    /// Rolls the window up into its titlebar, or down again.
    Shade,
    /// Shows the window on every workspace, or only its own.
    Stick,
    /// Minimizes the window.
    Minimize,
    /// Maximizes the window, or restores it.
    Maximize,
    /// Closes the window.
    Close,
    /// Opens the window's menu.
    Menu,
}

impl TitlebarButton {
    /// Returns what clicking the button does to the window; none for the
    /// menu button, which opens a menu instead.
    pub(crate) const fn action(self) -> Option<WindowManagerAction> {
        match self {
            Self::Shade => Some(WindowManagerAction::Shade),
            Self::Stick => Some(WindowManagerAction::Stick),
            Self::Minimize => Some(WindowManagerAction::Minimize),
            Self::Maximize => Some(WindowManagerAction::Maximize(Axes::Both)),
            Self::Close => Some(WindowManagerAction::Close),
            Self::Menu => None,
        }
    }

    /// Returns the button's picture, for a window that is `is_maximized` or
    /// not and `is_sticky` or not: the maximize and stick buttons show what
    /// clicking them would do.
    const fn glyph(self, (is_maximized, is_sticky): (bool, bool)) -> Glyph {
        match self {
            Self::Shade => Glyph::Shade,
            Self::Stick if is_sticky => Glyph::Stuck,
            Self::Stick => Glyph::Stick,
            Self::Menu => Glyph::Menu,
            Self::Minimize => Glyph::Minimize,
            Self::Maximize if is_maximized => Glyph::Restore,
            Self::Maximize => Glyph::Maximize,
            Self::Close => Glyph::Close,
        }
    }
}

/// The buttons on either side of a titlebar's title.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TitlebarLayout {
    /// Left of the title, left to right.
    pub(crate) left: Vec<TitlebarButton>,
    /// Right of the title, left to right.
    pub(crate) right: Vec<TitlebarButton>,
}

/// Which part of a frame is under a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FramePart {
    /// The titlebar, at this point in the titlebar's own pixels.
    Titlebar(vaht_ui::Point),
    /// A border line, and the edges dragging it there resizes.
    Border(ResizeEdges),
    /// The handle between its grips, or the line above it.
    Handle,
    /// A grip at one end of the handle, or the line above it.
    Grip(GripSide),
}

impl FramePart {
    /// Returns what dragging the part does unless a binding says
    /// otherwise: a border moves the window, as Fluxbox's does; the handle
    /// resizes the bottom edge and a grip its corner. The titlebar does
    /// nothing of its own (its bindings move the window).
    pub(crate) fn default_drag(self) -> Option<WindowDrag> {
        let bottom = |horizontal| WindowDrag::Resize {
            edges: ResizeEdges {
                horizontal,
                vertical: VerticalEdge::Bottom,
            },
        };
        match self {
            Self::Titlebar(_) => None,
            Self::Border(_) => Some(WindowDrag::Move),
            Self::Handle => Some(bottom(HorizontalEdge::None)),
            Self::Grip(GripSide::Left) => Some(bottom(HorizontalEdge::Left)),
            Self::Grip(GripSide::Right) => Some(bottom(HorizontalEdge::Right)),
        }
    }
}

/// Which end of the handle a grip is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GripSide {
    /// The left end.
    Left,
    /// The right end.
    Right,
}

/// Whether a frame shows its window's content, or only the titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameShape {
    /// Everything: the content, and the handle below it.
    Open,
    /// Rolled up: the frame closes beneath the titlebar.
    Shaded,
}

/// Where each part of a frame goes, in global coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameGeometry {
    /// Everything the frame covers, borders included.
    pub(crate) outer: Rectangle<i32, Logical>,
    /// The titlebar, exactly as wide as the content.
    pub(crate) titlebar: Rectangle<i32, Logical>,
    /// The handle with its grips, as wide as the content; empty when the
    /// frame has none.
    pub(crate) handle: Rectangle<i32, Logical>,
    /// How wide each grip is: none when the handle is too narrow for them,
    /// and is all handle.
    pub(crate) grip_width: Option<u32>,
    /// How wide the lines are.
    pub(crate) line_width: u32,
    /// How round the frame's corners are.
    pub(crate) corners: CornerRadii,
    /// The lines, in [`Self::LINES`]' order; a line the frame lacks is empty.
    pub(crate) borders: [Rectangle<i32, Logical>; 8],
}

impl FrameGeometry {
    /// The lines of a frame, in the order of [`Self::borders`]: top, under
    /// the titlebar, over the handle, bottom, left, right, then between
    /// each grip and the handle.
    pub(crate) const LINES: usize = 8;

    /// How wide a grip is, in pixels: Fluxbox's (`FbWinFrame`), so frames
    /// look as they did.
    const GRIP_WIDTH: u32 = 20;

    /// Returns the frame around `content` with the style's `metrics`, in
    /// `shape`: a shaded frame has no content and no handle.
    pub(crate) fn around(
        content: Rectangle<i32, Logical>,
        metrics: Metrics,
        shape: FrameShape,
    ) -> Self {
        let border = to_i32(metrics.border_width);
        let titlebar_height = to_i32(metrics.titlebar_height);
        let (handle_height, height) = match shape {
            FrameShape::Open => (to_i32(metrics.handle_height), content.size.h),
            FrameShape::Shaded => (0, 0),
        };
        // The handle brings its own line above it.
        let handle_span = if handle_height > 0 {
            handle_height + border
        } else {
            0
        };
        let (x, y, width) = (content.loc.x, content.loc.y, content.size.w);
        let outer_left = x - border;
        let outer_top = y - 2 * border - titlebar_height;
        let outer_width = width + 2 * border;
        let outer_height = height + titlebar_height + handle_span + 3 * border;
        let handle_top = y + height + border;
        let rectangle = |x, y, width, height| Rectangle::new((x, y).into(), (width, height).into());
        let corners = Self::corner_radii(
            metrics.frame_corners,
            (titlebar_height + border, handle_span),
            outer_width,
        );
        let radius = |corner| to_i32(corners.radius(corner));
        let (top_left, top_right) = (radius(Corner::TopLeft), radius(Corner::TopRight));
        let (bottom_left, bottom_right) = (radius(Corner::BottomLeft), radius(Corner::BottomRight));
        let handle = rectangle(x, handle_top, width, handle_height);
        let grip_width = Self::grip_width(handle, metrics.border_width);
        let grip_lines = grip_width.map_or([Rectangle::default(); 2], |grip| {
            let grip = to_i32(grip);
            [
                rectangle(x + grip, handle_top, border, handle_height),
                rectangle(x + width - grip - border, handle_top, border, handle_height),
            ]
        });
        Self {
            outer: rectangle(outer_left, outer_top, outer_width, outer_height),
            titlebar: rectangle(x, y - border - titlebar_height, width, titlebar_height),
            handle,
            grip_width,
            line_width: metrics.border_width,
            corners,
            // The outer lines stop short of rounded corners, where arcs join
            // them.
            borders: [
                rectangle(
                    outer_left + top_left,
                    outer_top,
                    outer_width - top_left - top_right,
                    border,
                ),
                rectangle(outer_left, y - border, outer_width, border),
                rectangle(
                    x,
                    y + height,
                    width,
                    if handle_height > 0 { border } else { 0 },
                ),
                rectangle(
                    outer_left + bottom_left,
                    y + height + handle_span,
                    outer_width - bottom_left - bottom_right,
                    border,
                ),
                rectangle(
                    outer_left,
                    outer_top + top_left,
                    border,
                    outer_height - top_left - bottom_left,
                ),
                rectangle(
                    x + width,
                    outer_top + top_right,
                    border,
                    outer_height - top_right - bottom_right,
                ),
                grip_lines[0],
                grip_lines[1],
            ],
        }
    }

    /// Returns the content area an open frame covering `outer` surrounds,
    /// the inverse of [`Self::around`]; at least one pixel each way.
    pub(crate) fn content_within(
        outer: Rectangle<i32, Logical>,
        metrics: Metrics,
    ) -> Rectangle<i32, Logical> {
        let border = to_i32(metrics.border_width);
        let titlebar_height = to_i32(metrics.titlebar_height);
        let handle_height = to_i32(metrics.handle_height);
        let handle_span = if handle_height > 0 {
            handle_height + border
        } else {
            0
        };
        Rectangle::new(
            (
                outer.loc.x + border,
                outer.loc.y + 2 * border + titlebar_height,
            )
                .into(),
            (
                (outer.size.w - 2 * border).max(1),
                (outer.size.h - titlebar_height - handle_span - 3 * border).max(1),
            )
                .into(),
        )
    }

    /// Returns where the corner square of `corner` is: as wide as its
    /// radius, in that corner of the frame; empty for a square corner.
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

    /// Returns how round the titlebar's corners are: the frame's top ones,
    /// just inside the lines.
    pub(crate) fn titlebar_corners(&self) -> CornerRadii {
        let inner = self.corners.inset(self.line_width);
        CornerRadii::SQUARE
            .with(Corner::TopLeft, inner.radius(Corner::TopLeft))
            .with(Corner::TopRight, inner.radius(Corner::TopRight))
    }

    /// Returns how round the handle's corners are: the frame's bottom ones,
    /// just inside the lines.
    pub(crate) fn handle_corners(&self) -> CornerRadii {
        let inner = self.corners.inset(self.line_width);
        CornerRadii::SQUARE
            .with(Corner::BottomLeft, inner.radius(Corner::BottomLeft))
            .with(Corner::BottomRight, inner.radius(Corner::BottomRight))
    }

    /// Returns the part of the frame at `point`, if any: none where a
    /// rounded corner was cut away.
    pub(crate) fn part_at(&self, point: Point<f64, Logical>) -> Option<FramePart> {
        let pixel = point.to_i32_floor();
        let inside = pixel - self.outer.loc;
        let size = vaht_ui::Size::new(to_u32(self.outer.size.w), to_u32(self.outer.size.h));
        if self
            .corners
            .cuts(size, vaht_ui::Point::new(inside.x, inside.y))
        {
            return None;
        }
        if self.titlebar.contains(pixel) {
            let inside = pixel - self.titlebar.loc;
            return Some(FramePart::Titlebar(vaht_ui::Point::new(inside.x, inside.y)));
        }
        if let Some(part) = self.handle_part_at(pixel) {
            return Some(part);
        }
        let corner = self.titlebar.size.h;
        self.borders
            .iter()
            .any(|border| border.contains(pixel))
            .then(|| FramePart::Border(ResizeEdges::for_border_point(self.outer, pixel, corner)))
    }

    /// Returns the handle's part at `pixel`, the line above it included:
    /// a grip, with the line beside it, or the handle.
    fn handle_part_at(&self, pixel: Point<i32, Logical>) -> Option<FramePart> {
        if self.handle.is_empty() {
            return None;
        }
        let line_above = self.borders[2];
        let row = Rectangle::new(
            line_above.loc,
            (self.handle.size.w, self.handle.size.h + line_above.size.h).into(),
        );
        if !row.contains(pixel) {
            return None;
        }
        let Some(grip_width) = self.grip_width else {
            return Some(FramePart::Handle);
        };
        let grip_span = to_i32(grip_width) + line_above.size.h;
        let across = pixel.x - row.loc.x;
        Some(if across < grip_span {
            FramePart::Grip(GripSide::Left)
        } else if across >= row.size.w - grip_span {
            FramePart::Grip(GripSide::Right)
        } else {
            FramePart::Handle
        })
    }

    /// Returns the style's corner radii as a frame `width` wide takes
    /// them: a top corner no rounder than `room`'s first part (the
    /// titlebar and the line above it) is tall, a bottom one than its
    /// second (the handle and the line below it; nothing without a handle),
    /// and none rounder than half the width.
    fn corner_radii(style: CornerRadii, room: (i32, i32), width: i32) -> CornerRadii {
        let (top, bottom) = room;
        <Corner as strum::IntoEnumIterator>::iter().fold(style, |radii, corner| {
            let room = if corner.is_top() { top } else { bottom };
            let most = to_u32(room.min(width / 2));
            radii.with(corner, style.radius(corner).min(most))
        })
    }

    /// Returns how wide `handle`'s grips are, with lines `border` wide
    /// beside them: none when they and their lines would not leave room for
    /// any handle between them, as Fluxbox draws it.
    fn grip_width(handle: Rectangle<i32, Logical>, border: u32) -> Option<u32> {
        let needed = 2 * (Self::GRIP_WIDTH + border);
        (!handle.is_empty() && to_u32(handle.size.w) > needed).then_some(Self::GRIP_WIDTH)
    }
}

/// A window's frame: what its titlebar shows, laid out and painted.
///
/// The pixels stay here; the compositor uploads them for the renderer
/// ([`crate::scene::FrameTextures`]), so the window manager never sees a
/// renderer.
#[derive(Debug)]
pub(crate) struct Frame {
    /// The titlebar's content, layout and pixels.
    titlebar: Panel<Titlebar>,
    /// The handle's content, layout and pixels.
    handle: Panel<HandleBar>,
    /// Counts the frame while it exists.
    _tracked: Tracked,
}

impl Frame {
    /// Creates the frame of an unfocused, untitled window with no content
    /// yet, its titlebar's buttons laid out as `buttons`.
    pub(crate) fn new(buttons: TitlebarLayout, ledger: &ResourceLedger) -> Self {
        Self {
            titlebar: Panel::new(
                Titlebar {
                    title: String::new(),
                    icon: None,
                    is_focused: false,
                    is_maximized: false,
                    is_sticky: false,
                    buttons,
                },
                vaht_ui::Size::default(),
            ),
            handle: Panel::new(
                HandleBar {
                    is_focused: false,
                    grips: None,
                },
                vaht_ui::Size::default(),
            ),
            _tracked: ledger.track(ResourceKind::Frame),
        }
    }

    /// Returns the titlebar panel: its pixels, and how new they are.
    pub(crate) const fn titlebar(&self) -> &Panel<Titlebar> {
        &self.titlebar
    }

    /// Returns the handle panel: its pixels, and how new they are.
    pub(crate) const fn handle(&self) -> &Panel<HandleBar> {
        &self.handle
    }

    /// Returns how the frame looks: focused or not.
    pub(crate) fn state(&self) -> WidgetState {
        self.titlebar.content().state()
    }

    /// Returns whether a titlebar button is at `point`, in the titlebar's
    /// own pixels.
    pub(crate) fn has_button_at(&self, point: vaht_ui::Point) -> bool {
        matches!(self.titlebar.hit_at(point), Some(TitlebarHit::Button(_)))
    }

    /// Holds down the titlebar button at `point` (in the titlebar's own
    /// pixels), if there is one there; returns whether there was.
    pub(crate) fn press_button(&mut self, point: vaht_ui::Point) -> bool {
        let is_button = self.has_button_at(point);
        if is_button {
            self.titlebar.press(point);
        }
        is_button
    }

    /// Lets go of the held titlebar button with the pointer at `point`;
    /// returns the button if the pointer is still on it, which clicks it.
    pub(crate) fn release_button(&mut self, point: vaht_ui::Point) -> Option<TitlebarButton> {
        match self.titlebar.release(point)? {
            TitlebarHit::Button(button) => Some(button),
            TitlebarHit::Label => None,
        }
    }

    /// Shows `title`.
    pub(crate) fn set_title(&mut self, title: &str) {
        if self.titlebar.content().title != title {
            self.titlebar
                .update(|content| title.clone_into(&mut content.title));
        }
    }

    /// Shows `icon` left of the title, or none.
    pub(crate) fn set_icon(&mut self, icon: Option<Icon>) {
        if self.titlebar.content().icon != icon {
            self.titlebar.update(|content| content.icon = icon);
        }
    }

    /// Lays the titlebar's buttons out as `buttons`.
    pub(crate) fn set_buttons(&mut self, buttons: &TitlebarLayout) {
        if &self.titlebar.content().buttons != buttons {
            self.titlebar
                .update(|content| content.buttons.clone_from(buttons));
        }
    }

    /// Shows the window as focused or not.
    pub(crate) fn set_focused(&mut self, is_focused: bool) {
        if self.titlebar.content().is_focused != is_focused {
            self.titlebar
                .update(|content| content.is_focused = is_focused);
            self.handle
                .update(|content| content.is_focused = is_focused);
        }
    }

    /// Returns whether the frame shows its window as maximized.
    #[cfg(test)]
    pub(crate) fn shows_maximized(&self) -> bool {
        self.titlebar.content().is_maximized
    }

    /// Shows the window as maximized or not, which its maximize button
    /// shows by being a restore button.
    pub(crate) fn set_maximized(&mut self, is_maximized: bool) {
        if self.titlebar.content().is_maximized != is_maximized {
            self.titlebar
                .update(|content| content.is_maximized = is_maximized);
        }
    }

    /// Shows the window as on every workspace or not, which its stick button
    /// shows by being an unstick button.
    pub(crate) fn set_sticky(&mut self, is_sticky: bool) {
        if self.titlebar.content().is_sticky != is_sticky {
            self.titlebar
                .update(|content| content.is_sticky = is_sticky);
        }
    }

    /// Marks the frame for painting again, as when the style changed.
    pub(crate) fn invalidate(&mut self) {
        self.titlebar.invalidate();
        self.handle.invalidate();
    }

    /// Paints the titlebar and the handle to fit `geometry`, if anything
    /// changed.
    pub(crate) fn paint(
        &mut self,
        geometry: &FrameGeometry,
        style: &Style,
        typesetter: &mut Typesetter,
    ) {
        let size = geometry.titlebar.size;
        self.titlebar
            .resize(vaht_ui::Size::new(to_u32(size.w), to_u32(size.h)));
        self.titlebar.paint(style, typesetter);
        self.titlebar.set_corners(geometry.titlebar_corners());
        self.handle.set_corners(geometry.handle_corners());
        let grips = geometry.grip_width.map(|width| GripLayout {
            width,
            line_width: geometry.line_width,
        });
        if self.handle.content().grips != grips {
            self.handle.update(|content| content.grips = grips);
        }
        let size = geometry.handle.size;
        self.handle
            .resize(vaht_ui::Size::new(to_u32(size.w), to_u32(size.h)));
        self.handle.paint(style, typesetter);
    }
}

/// A titlebar's content: the title, the buttons, and the window's focus.
#[derive(Debug)]
pub(crate) struct Titlebar {
    /// The window's title.
    title: String,
    /// The window's icon, shown left of the title.
    icon: Option<Icon>,
    /// Whether the window has keyboard focus.
    is_focused: bool,
    /// Whether the window is maximized, so its maximize button restores it.
    is_maximized: bool,
    /// Whether the window is on every workspace, so its stick button unsticks.
    is_sticky: bool,
    /// The buttons either side of the title.
    buttons: TitlebarLayout,
}

impl PanelContent for Titlebar {
    type Hit = TitlebarHit;

    fn build(&self) -> Widget<TitlebarHit> {
        let button = |button: &TitlebarButton| {
            Widget::glyph(
                StyleElement::Button,
                button.glyph((self.is_maximized, self.is_sticky)),
            )
            .with_button(TitlebarHit::Button(*button))
        };
        let label = Widget::text(StyleElement::Label, self.title.clone())
            .with_size(SizePolicy::Fill { weight: 1 })
            .with_hit(TitlebarHit::Label);
        let label = match &self.icon {
            Some(icon) => label.with_icon(WidgetIcon {
                icon: icon.clone(),
                emphasis: IconEmphasis::Full,
            }),
            None => label,
        };
        let children = self
            .buttons
            .left
            .iter()
            .map(button)
            .chain([label])
            .chain(self.buttons.right.iter().map(button))
            .collect();
        Widget::row(StyleElement::Titlebar, children)
    }

    fn state(&self) -> WidgetState {
        if self.is_focused {
            WidgetState::Focused
        } else {
            WidgetState::Unfocused
        }
    }
}

/// A handle's content: a strip with a grip at each end.
#[derive(Debug)]
pub(crate) struct HandleBar {
    /// Whether the window has keyboard focus.
    is_focused: bool,
    /// Where the grips go; none when the handle is too narrow for them.
    grips: Option<GripLayout>,
}

/// How a handle's grips are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GripLayout {
    /// How wide each grip is.
    width: u32,
    /// How wide the line between each grip and the handle is; the frame
    /// draws it there.
    line_width: u32,
}

impl PanelContent for HandleBar {
    type Hit = ();

    fn build(&self) -> Widget<()> {
        let children = self.grips.map_or_else(Vec::new, |grips| {
            let grip =
                || Widget::fill(StyleElement::Grip).with_size(SizePolicy::Fixed(grips.width));
            let line = || {
                Widget::fill(StyleElement::Handle).with_size(SizePolicy::Fixed(grips.line_width))
            };
            vec![
                grip(),
                line(),
                Widget::fill(StyleElement::Handle),
                line(),
                grip(),
            ]
        });
        Widget::row(StyleElement::Handle, children)
    }

    fn state(&self) -> WidgetState {
        if self.is_focused {
            WidgetState::Focused
        } else {
            WidgetState::Unfocused
        }
    }
}

/// Converts a pixel extent to a signed coordinate.
fn to_i32(extent: u32) -> i32 {
    i32::try_from(extent).expect("frame sizes are far below i32::MAX")
}

/// Converts a non-negative coordinate extent to a size.
fn to_u32(extent: i32) -> u32 {
    u32::try_from(extent).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A titlebar 20 pixels tall, lines 1 pixel wide and a handle 1 tall.
    const METRICS: Metrics = Metrics {
        titlebar_height: 20,
        border_width: 1,
        handle_height: 1,
        taskbar_height: 20,
        frame_corners: CornerRadii::SQUARE,
        menu: vaht_ui::style::MenuMetrics {
            title_height: 20,
            item_height: 20,
            border_width: 1,
            corners: CornerRadii::SQUARE,
            bullet: vaht_ui::style::Bullet::Triangle,
            bullet_side: vaht_ui::MarkSide::Right,
        },
    };

    /// The frame around a 100 by 50 window whose content is at (10, 40).
    fn frame() -> FrameGeometry {
        FrameGeometry::around(
            Rectangle::new((10, 40).into(), (100, 50).into()),
            METRICS,
            FrameShape::Open,
        )
    }

    #[test]
    fn content_within_undoes_around() {
        let content = Rectangle::new((40, 60).into(), (300, 200).into());
        let outer = FrameGeometry::around(content, METRICS, FrameShape::Open).outer;
        assert_eq!(FrameGeometry::content_within(outer, METRICS), content);
    }

    #[test]
    fn the_titlebar_sits_on_the_content_with_a_border_between() {
        assert_eq!(
            frame().titlebar,
            Rectangle::new((10, 19).into(), (100, 20).into())
        );
        assert_eq!(
            frame().borders[1],
            Rectangle::new((9, 39).into(), (102, 1).into())
        );
    }

    #[test]
    fn the_borders_enclose_titlebar_content_and_handle() {
        let [top, _, over_handle, bottom, left, right, ..] = frame().borders;
        assert_eq!(top.loc, (9, 18).into());
        assert_eq!(
            over_handle,
            Rectangle::new((10, 90).into(), (100, 1).into())
        );
        assert_eq!(
            frame().handle,
            Rectangle::new((10, 91).into(), (100, 1).into())
        );
        assert_eq!(bottom.loc, (9, 92).into());
        assert_eq!(left.size.h, 75, "1 + 20 + 1 + 50 + 1 + 1 + 1");
        assert_eq!(right.loc.x, 110);
    }

    #[test]
    fn lines_part_the_grips_from_the_handle() {
        let [.., left_line, right_line] = frame().borders;
        assert_eq!(left_line, Rectangle::new((30, 91).into(), (1, 1).into()));
        assert_eq!(right_line, Rectangle::new((89, 91).into(), (1, 1).into()));
    }

    #[test]
    fn a_handle_too_narrow_for_grips_is_all_handle() {
        let narrow = FrameGeometry::around(
            Rectangle::new((10, 40).into(), (42, 50).into()),
            METRICS,
            FrameShape::Open,
        );
        assert_eq!(narrow.grip_width, None);
        assert!(narrow.borders[6].is_empty() && narrow.borders[7].is_empty());
        assert_eq!(narrow.part_at((12.0, 91.0).into()), Some(FramePart::Handle));
    }

    #[test]
    fn a_shaded_frame_closes_beneath_its_titlebar_without_a_handle() {
        let shaded = FrameGeometry::around(
            Rectangle::new((10, 40).into(), (100, 50).into()),
            METRICS,
            FrameShape::Shaded,
        );
        assert!(shaded.handle.is_empty());
        assert_eq!(shaded.outer.size.h, 23, "1 + 20 + 1 + 1");
    }

    #[test]
    fn without_a_handle_height_the_frame_has_none() {
        let metrics = Metrics {
            handle_height: 0,
            ..METRICS
        };
        let content = Rectangle::new((10, 40).into(), (100, 50).into());
        let geometry = FrameGeometry::around(content, metrics, FrameShape::Open);
        assert!(geometry.handle.is_empty());
        assert_eq!(geometry.outer.size.h, 73, "1 + 20 + 1 + 50 + 1");
        assert_eq!(
            FrameGeometry::content_within(geometry.outer, metrics),
            content
        );
    }

    #[test]
    fn points_are_classified_by_the_part_they_fall_on() {
        let frame = frame();
        assert_eq!(
            frame.part_at((15.0, 25.0).into()),
            Some(FramePart::Titlebar(vaht_ui::Point::new(5, 6)))
        );
        assert_eq!(
            frame.part_at((9.0, 60.0).into()),
            Some(FramePart::Border(ResizeEdges {
                horizontal: HorizontalEdge::Left,
                vertical: VerticalEdge::None,
            }))
        );
        assert_eq!(
            frame.part_at((50.0, 60.0).into()),
            None,
            "the content is not frame"
        );
        assert_eq!(frame.part_at((50.0, 91.0).into()), Some(FramePart::Handle));
        assert_eq!(
            frame.part_at((12.0, 91.0).into()),
            Some(FramePart::Grip(GripSide::Left))
        );
        assert_eq!(
            frame.part_at((100.0, 90.0).into()),
            Some(FramePart::Grip(GripSide::Right)),
            "the line above a grip is the grip's"
        );
    }

    /// The test frame, its style rounding every corner by 8 pixels.
    fn rounded(shape: FrameShape) -> FrameGeometry {
        let metrics = Metrics {
            frame_corners: CornerRadii::of(8, <Corner as strum::IntoEnumIterator>::iter()),
            ..METRICS
        };
        FrameGeometry::around(
            Rectangle::new((10, 40).into(), (100, 50).into()),
            metrics,
            shape,
        )
    }

    #[test]
    fn corners_round_within_the_titlebar_and_the_handle() {
        let frame = rounded(FrameShape::Open);
        assert_eq!(frame.corners.radius(Corner::TopLeft), 8);
        assert_eq!(
            frame.corners.radius(Corner::BottomRight),
            2,
            "no rounder than the handle and its line"
        );
        let shaded = rounded(FrameShape::Shaded);
        assert_eq!(
            shaded.corners.radius(Corner::BottomLeft),
            0,
            "no handle, no round bottom"
        );
    }

    #[test]
    fn lines_stop_short_of_rounded_corners() {
        let [top, _, _, bottom, left, ..] = rounded(FrameShape::Open).borders;
        assert_eq!(top, Rectangle::new((17, 18).into(), (86, 1).into()));
        assert_eq!(bottom, Rectangle::new((11, 92).into(), (98, 1).into()));
        assert_eq!(
            left,
            Rectangle::new((9, 26).into(), (1, 65).into()),
            "75 tall, less 8 above and 2 below"
        );
    }

    #[test]
    fn the_cut_corner_is_no_part_of_the_frame() {
        let frame = rounded(FrameShape::Open);
        assert_eq!(frame.part_at((9.0, 18.0).into()), None, "cut away");
        assert!(matches!(
            frame.part_at((17.0, 18.0).into()),
            Some(FramePart::Border(_))
        ));
        assert_eq!(
            frame.titlebar_corners(),
            CornerRadii::of(7, [Corner::TopLeft, Corner::TopRight]),
            "just inside the line"
        );
    }

    #[test]
    fn dragging_a_border_moves_and_the_handle_or_a_grip_resizes_the_bottom() {
        let bottom = |horizontal| {
            Some(WindowDrag::Resize {
                edges: ResizeEdges {
                    horizontal,
                    vertical: VerticalEdge::Bottom,
                },
            })
        };
        let top = ResizeEdges {
            horizontal: HorizontalEdge::None,
            vertical: VerticalEdge::Top,
        };
        assert_eq!(
            FramePart::Border(top).default_drag(),
            Some(WindowDrag::Move)
        );
        assert_eq!(
            FramePart::Handle.default_drag(),
            bottom(HorizontalEdge::None)
        );
        assert_eq!(
            FramePart::Grip(GripSide::Left).default_drag(),
            bottom(HorizontalEdge::Left)
        );
        assert_eq!(
            FramePart::Grip(GripSide::Right).default_drag(),
            bottom(HorizontalEdge::Right)
        );
        assert_eq!(
            FramePart::Titlebar(vaht_ui::Point::new(0, 0)).default_drag(),
            None
        );
    }

    #[test]
    fn the_titlebar_reports_its_buttons() {
        let titlebar = Titlebar {
            title: "a window".to_owned(),
            icon: None,
            is_focused: true,
            is_maximized: false,
            is_sticky: false,
            buttons: TitlebarLayout {
                left: vec![TitlebarButton::Stick],
                right: vec![
                    TitlebarButton::Shade,
                    TitlebarButton::Minimize,
                    TitlebarButton::Maximize,
                    TitlebarButton::Close,
                ],
            },
        };
        let root = titlebar.build();
        let vaht_ui::widget::WidgetKind::Row(children) = root.kind else {
            panic!("a titlebar is a row");
        };
        let hits: Vec<_> = children.iter().map(|child| child.hit).collect();
        assert_eq!(
            hits,
            [
                Some(TitlebarHit::Button(TitlebarButton::Stick)),
                Some(TitlebarHit::Label),
                Some(TitlebarHit::Button(TitlebarButton::Shade)),
                Some(TitlebarHit::Button(TitlebarButton::Minimize)),
                Some(TitlebarHit::Button(TitlebarButton::Maximize)),
                Some(TitlebarHit::Button(TitlebarButton::Close)),
            ]
        );
    }

    /// Returns the picture of the button for `button` in a titlebar of a
    /// window that is `is_maximized` or not.
    fn glyph_of(button: TitlebarButton, is_maximized: bool) -> Glyph {
        let titlebar = Titlebar {
            title: String::new(),
            icon: None,
            is_focused: true,
            is_maximized,
            is_sticky: false,
            buttons: TitlebarLayout {
                left: Vec::new(),
                right: vec![button],
            },
        };
        let vaht_ui::widget::WidgetKind::Row(children) = titlebar.build().kind else {
            panic!("a titlebar is a row");
        };
        let vaht_ui::widget::WidgetKind::Glyph(glyph) = children[1].kind else {
            panic!("a button is a glyph");
        };
        glyph
    }

    #[test]
    fn the_maximize_button_is_a_restore_button_while_the_window_is_maximized() {
        assert_eq!(glyph_of(TitlebarButton::Maximize, false), Glyph::Maximize);
        assert_eq!(glyph_of(TitlebarButton::Maximize, true), Glyph::Restore);
    }

    /// Returns the picture of `button` for a window that is `is_sticky` or not.
    fn glyph_when_sticky(button: TitlebarButton, is_sticky: bool) -> Glyph {
        button.glyph((false, is_sticky))
    }

    #[test]
    fn the_stick_button_shows_stuck_while_the_window_is_on_every_workspace() {
        assert_eq!(
            glyph_when_sticky(TitlebarButton::Stick, false),
            Glyph::Stick
        );
        assert_eq!(glyph_when_sticky(TitlebarButton::Stick, true), Glyph::Stuck);
        assert_eq!(glyph_when_sticky(TitlebarButton::Close, true), Glyph::Close);
    }

    #[test]
    fn the_menu_button_opens_a_menu_rather_than_acting_on_the_window() {
        assert_eq!(TitlebarButton::Menu.action(), None);
        assert_eq!(glyph_when_sticky(TitlebarButton::Menu, false), Glyph::Menu);
        assert!(TitlebarButton::Close.action().is_some());
    }

    #[test]
    fn the_other_buttons_look_the_same_whether_or_not_the_window_is_maximized() {
        for button in [
            TitlebarButton::Shade,
            TitlebarButton::Stick,
            TitlebarButton::Minimize,
            TitlebarButton::Close,
        ] {
            assert_eq!(
                glyph_of(button, false),
                glyph_of(button, true),
                "{button:?}"
            );
        }
    }
}
