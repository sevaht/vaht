//! What vaht draws around a window: a titlebar and border lines.
//!
//! The titlebar is a [`Panel`]; vaht-ui lays it out, paints it and hit-tests
//! it, and this module only says what it shows (01 §4.3). The borders are
//! plain colored rectangles, which cost no pixel buffers. The frame is never
//! one big buffer around the window, which would cost megabytes per window.
//!
//! Geometry is derived, never stored: [`FrameGeometry::around`] computes every
//! part from the window's content area and the theme's metrics, so the frame
//! cannot drift from the window it surrounds.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{
    Buffer as BufferCoordinates, Logical, Physical, Point, Rectangle, Scale, Transform,
};
use vaht_ui::texture::Fill;
use vaht_ui::theme::Metrics;
use vaht_ui::{
    Glyph, Panel, PanelContent, SizePolicy, Theme, ThemeElement, Typesetter, Widget, WidgetState,
};

use super::resize::ResizeEdges;
use crate::config::Action;
use crate::resources::{ResourceKind, ResourceLedger, Tracked};
use crate::scene::SceneElement;
use crate::window::Axes;

/// What clicking a titlebar can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TitlebarHit {
    /// The title: dragging it will move the window.
    Label,
    /// A button.
    Button(TitlebarButton),
}

/// A titlebar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TitlebarButton {
    /// Rolls the window up into its titlebar, or down again.
    Shade,
    /// Minimizes the window.
    Minimize,
    /// Maximizes the window, or restores it.
    Maximize,
    /// Closes the window.
    Close,
}

impl TitlebarButton {
    /// Returns what clicking the button does.
    pub(crate) const fn action(self) -> Action {
        match self {
            Self::Shade => Action::Shade,
            Self::Minimize => Action::Minimize,
            Self::Maximize => Action::Maximize(Axes::Both),
            Self::Close => Action::Close,
        }
    }

    /// Returns the button's picture.
    const fn glyph(self) -> Glyph {
        match self {
            Self::Shade => Glyph::Shade,
            Self::Minimize => Glyph::Minimize,
            Self::Maximize => Glyph::Maximize,
            Self::Close => Glyph::Close,
        }
    }
}

/// Which part of a frame is under a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FramePart {
    /// The titlebar, at this point in the titlebar's own pixels.
    Titlebar(vaht_ui::Point),
    /// A border line, and the edges dragging it there resizes.
    Border(ResizeEdges),
}

/// Where each part of a frame goes, in global coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameGeometry {
    /// Everything the frame covers, borders included.
    pub(crate) outer: Rectangle<i32, Logical>,
    /// The titlebar, exactly as wide as the content.
    pub(crate) titlebar: Rectangle<i32, Logical>,
    /// Top, separator (between titlebar and content), bottom, left, right.
    pub(crate) borders: [Rectangle<i32, Logical>; 5],
}

impl FrameGeometry {
    /// Returns the frame around `content` with the theme's `metrics`.
    pub(crate) fn around(content: Rectangle<i32, Logical>, metrics: Metrics) -> Self {
        let border = to_i32(metrics.border_width);
        let titlebar_height = to_i32(metrics.titlebar_height);
        let (x, y) = (content.loc.x, content.loc.y);
        let (width, height) = (content.size.w, content.size.h);
        let outer_left = x - border;
        let outer_top = y - 2 * border - titlebar_height;
        let outer_width = width + 2 * border;
        let outer_height = height + titlebar_height + 3 * border;
        let rectangle = |x, y, width, height| Rectangle::new((x, y).into(), (width, height).into());
        Self {
            outer: rectangle(outer_left, outer_top, outer_width, outer_height),
            titlebar: rectangle(x, y - border - titlebar_height, width, titlebar_height),
            borders: [
                rectangle(outer_left, outer_top, outer_width, border),
                rectangle(outer_left, y - border, outer_width, border),
                rectangle(outer_left, y + height, outer_width, border),
                rectangle(outer_left, outer_top, border, outer_height),
                rectangle(x + width, outer_top, border, outer_height),
            ],
        }
    }

    /// Returns the content area a frame covering `outer` surrounds, the
    /// inverse of [`Self::around`]; at least one pixel each way.
    pub(crate) fn content_within(
        outer: Rectangle<i32, Logical>,
        metrics: Metrics,
    ) -> Rectangle<i32, Logical> {
        let border = to_i32(metrics.border_width);
        let titlebar_height = to_i32(metrics.titlebar_height);
        Rectangle::new(
            (
                outer.loc.x + border,
                outer.loc.y + 2 * border + titlebar_height,
            )
                .into(),
            (
                (outer.size.w - 2 * border).max(1),
                (outer.size.h - titlebar_height - 3 * border).max(1),
            )
                .into(),
        )
    }

    /// Returns the part of the frame at `point`, if any.
    pub(crate) fn part_at(&self, point: Point<f64, Logical>) -> Option<FramePart> {
        let pixel = point.to_i32_floor();
        if self.titlebar.contains(pixel) {
            let inside = pixel - self.titlebar.loc;
            return Some(FramePart::Titlebar(vaht_ui::Point::new(inside.x, inside.y)));
        }
        let corner = self.titlebar.size.h;
        self.borders
            .iter()
            .any(|border| border.contains(pixel))
            .then(|| FramePart::Border(ResizeEdges::for_border_point(self.outer, pixel, corner)))
    }
}

/// A window's frame: its titlebar panel and its borders' buffers.
#[derive(Debug)]
pub(crate) struct Frame {
    /// The titlebar's content, layout and pixels.
    titlebar: Panel<Titlebar>,
    /// The titlebar's pixels as the renderer takes them.
    titlebar_buffer: MemoryRenderBuffer,
    /// The titlebar generation last copied into the buffer.
    uploaded_generation: Option<u64>,
    /// One colored buffer per border line, in [`FrameGeometry::borders`]'s order.
    borders: [SolidColorBuffer; 5],
    /// Counts the frame while it exists.
    _tracked: Tracked,
}

impl Frame {
    /// The titlebar's buttons, left to right, as my fluxland setup has
    /// them; `settings.toml` will choose them.
    const BUTTONS: [TitlebarButton; 4] = [
        TitlebarButton::Shade,
        TitlebarButton::Minimize,
        TitlebarButton::Maximize,
        TitlebarButton::Close,
    ];

    /// Creates the frame of an unfocused, untitled window with no content yet.
    pub(crate) fn new(ledger: &ResourceLedger) -> Self {
        Self {
            titlebar: Panel::new(
                Titlebar {
                    title: String::new(),
                    is_focused: false,
                },
                vaht_ui::Size::default(),
            ),
            titlebar_buffer: MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                (1, 1),
                1,
                Transform::Normal,
                None,
            ),
            uploaded_generation: None,
            borders: std::array::from_fn(|_| SolidColorBuffer::default()),
            _tracked: ledger.track(ResourceKind::Frame),
        }
    }

    /// Holds down the titlebar button at `point` (in the titlebar's own
    /// pixels), if there is one there; returns whether there was.
    pub(crate) fn press_button(&mut self, point: vaht_ui::Point) -> bool {
        let is_button = matches!(self.titlebar.hit_at(point), Some(TitlebarHit::Button(_)));
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

    /// Shows `title`; returns whether it changed.
    pub(crate) fn set_title(&mut self, title: &str) -> bool {
        let changed = self.titlebar.content().title != title;
        if changed {
            self.titlebar
                .update(|content| title.clone_into(&mut content.title));
        }
        changed
    }

    /// Shows the window as focused or not; returns whether that changed.
    pub(crate) fn set_focused(&mut self, is_focused: bool) -> bool {
        let changed = self.titlebar.content().is_focused != is_focused;
        if changed {
            self.titlebar
                .update(|content| content.is_focused = is_focused);
        }
        changed
    }

    /// Paints the titlebar to fit `geometry` if anything changed, and copies
    /// new pixels for the renderer; recolors the borders.
    pub(crate) fn prepare(
        &mut self,
        geometry: &FrameGeometry,
        theme: &Theme,
        typesetter: &mut Typesetter,
    ) {
        let size = geometry.titlebar.size;
        self.titlebar
            .resize(vaht_ui::Size::new(to_u32(size.w), to_u32(size.h)));
        self.titlebar.paint(theme, typesetter);
        self.upload_titlebar();
        let border_color = solid_color(theme, self.titlebar.content().state());
        for (buffer, rectangle) in self.borders.iter_mut().zip(geometry.borders) {
            buffer.update(rectangle.size, border_color);
        }
    }

    /// Returns the elements that draw the frame at `geometry`, topmost first.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        geometry: &FrameGeometry,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        if self.titlebar.pixels().is_some() {
            let location: Point<i32, Physical> =
                geometry.titlebar.loc.to_physical_precise_round(scale);
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &self.titlebar_buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            ) {
                Ok(element) => elements.push(SceneElement::Panel(element)),
                Err(error) => tracing::warn!(?error, "could not draw a titlebar"),
            }
        }
        for (buffer, rectangle) in self.borders.iter().zip(geometry.borders) {
            if !rectangle.is_empty() {
                elements.push(SceneElement::Solid(SolidColorRenderElement::from_buffer(
                    buffer,
                    rectangle.loc.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                )));
            }
        }
        elements
    }

    /// Copies the titlebar's pixels into the renderer's buffer if they are
    /// newer than the last copy.
    fn upload_titlebar(&mut self) {
        let Some(pixels) = self.titlebar.pixels() else {
            return;
        };
        let generation = self.titlebar.generation();
        if self.uploaded_generation == Some(generation) {
            return;
        }
        let size: smithay::utils::Size<i32, BufferCoordinates> =
            (to_i32(pixels.width()), to_i32(pixels.height())).into();
        let mut context = self.titlebar_buffer.render();
        context.resize(size);
        let drawn: Result<(), std::convert::Infallible> = context.draw(|memory| {
            // tiny-skia is premultiplied RGBA; Wayland's ARGB8888 is
            // premultiplied BGRA in memory.
            let (targets, _) = memory.as_chunks_mut::<4>();
            for (target, source) in targets.iter_mut().zip(pixels.pixels()) {
                *target = [source.blue(), source.green(), source.red(), source.alpha()];
            }
            Ok(vec![Rectangle::from_size(size)])
        });
        match drawn {
            Ok(()) => self.uploaded_generation = Some(generation),
            Err(never) => match never {},
        }
    }
}

/// A titlebar's content: the title, the buttons, and the window's focus.
#[derive(Debug)]
pub(crate) struct Titlebar {
    /// The window's title.
    title: String,
    /// Whether the window has keyboard focus.
    is_focused: bool,
}

impl PanelContent for Titlebar {
    type Hit = TitlebarHit;

    fn build(&self) -> Widget<TitlebarHit> {
        let mut children = vec![
            Widget::text(ThemeElement::Label, self.title.clone())
                .with_size(SizePolicy::Fill { weight: 1 })
                .with_hit(TitlebarHit::Label),
        ];
        children.extend(Frame::BUTTONS.iter().map(|button| {
            Widget::glyph(ThemeElement::Button, button.glyph())
                .with_button(TitlebarHit::Button(*button))
        }));
        Widget::row(ThemeElement::Titlebar, children)
    }

    fn state(&self) -> WidgetState {
        if self.is_focused {
            WidgetState::Focused
        } else {
            WidgetState::Unfocused
        }
    }
}

/// Returns the border color the theme gives windows in `state`: the
/// border's fill, or the start of its gradient.
fn solid_color(theme: &Theme, state: WidgetState) -> [f32; 4] {
    let color = match theme.style(ThemeElement::Border, state).texture.fill {
        Fill::Solid(color) | Fill::Gradient { from: color, .. } => color,
    };
    [color.red, color.green, color.blue, color.alpha].map(|channel| f32::from(channel) / 255.0)
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

    /// A titlebar 20 pixels tall and borders 1 pixel wide.
    const METRICS: Metrics = Metrics {
        titlebar_height: 20,
        border_width: 1,
        handle_height: 1,
    };

    /// The frame around a 100 by 50 window whose content is at (10, 40).
    fn frame() -> FrameGeometry {
        FrameGeometry::around(Rectangle::new((10, 40).into(), (100, 50).into()), METRICS)
    }

    #[test]
    fn content_within_undoes_around() {
        let content = Rectangle::new((40, 60).into(), (300, 200).into());
        let outer = FrameGeometry::around(content, METRICS).outer;
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
    fn the_borders_enclose_titlebar_and_content() {
        let [top, _, bottom, left, right] = frame().borders;
        assert_eq!(top.loc, (9, 18).into());
        assert_eq!(bottom.loc, (9, 90).into());
        assert_eq!(left.size.h, 73, "1 + 20 + 1 + 50 + 1");
        assert_eq!(right.loc.x, 110);
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
                horizontal: super::super::HorizontalEdge::Left,
                vertical: super::super::VerticalEdge::None,
            }))
        );
        assert_eq!(
            frame.part_at((50.0, 60.0).into()),
            None,
            "the content is not frame"
        );
    }

    #[test]
    fn the_titlebar_reports_its_buttons() {
        let titlebar = Titlebar {
            title: "a window".to_owned(),
            is_focused: true,
        };
        let root = titlebar.build();
        let vaht_ui::widget::WidgetKind::Row(children) = root.kind else {
            panic!("a titlebar is a row");
        };
        let hits: Vec<_> = children.iter().map(|child| child.hit).collect();
        assert_eq!(
            hits,
            [
                Some(TitlebarHit::Label),
                Some(TitlebarHit::Button(TitlebarButton::Shade)),
                Some(TitlebarHit::Button(TitlebarButton::Minimize)),
                Some(TitlebarHit::Button(TitlebarButton::Maximize)),
                Some(TitlebarHit::Button(TitlebarButton::Close)),
            ]
        );
    }
}
