//! A tooltip: the text for what the pointer rests on, after a while.
//!
//! As fluxland's: a window list entry whose title does not fit shows the
//! whole title, and a tray item its own tooltip (or title), a line each,
//! in the style's `tooltip` style, its corners as round as that says. It
//! is centered over the pointer, 10
//! pixels above it, or below it when there is no room above, and kept on
//! the output. Moving to something else hides it and waits again; a press
//! hides it until the pointer moves to something else.

use std::time::Duration;

use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Point, Rectangle, Scale};
use vaht_ui::{
    Panel, PanelContent, SizePolicy, Style, StyleElement, Typesetter, Widget, WidgetState,
};

use crate::Compositor;
use crate::event_source::Timer;
use crate::resources::ResourceLedger;
use crate::scene::{Outline, PanelTexture, SceneElement, SceneRenderer};
use crate::window_manager::Area;

/// The tooltip: what it is to say, and whether it shows.
#[derive(Debug, Default)]
pub(crate) struct Tooltip {
    /// The text for what the pointer rests on, if anything.
    wanted: Option<String>,
    /// Shows the tooltip once the pointer has rested long enough.
    timer: Option<Timer<Compositor>>,
    /// The tooltip shown, if one is.
    shown: Option<Shown>,
}

/// A tooltip shown.
#[derive(Debug)]
struct Shown {
    /// Where it is, its border included.
    area: Area,
    /// How wide its border is.
    border: i32,
    /// Its text's pixels.
    texture: PanelTexture,
    /// Its border.
    outline: Outline,
    /// How opaque it is drawn, as the style says.
    opacity: f32,
}

/// What a panel of lines of text shows (a tooltip, the configuration
/// warning): its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lines {
    /// The lines of text.
    pub(crate) texts: Vec<String>,
    /// How tall each line is.
    pub(crate) line_height: u32,
    /// What styles it.
    pub(crate) element: StyleElement,
}

impl PanelContent for Lines {
    type Hit = ();

    fn build(&self) -> Widget<()> {
        let rows = self
            .texts
            .iter()
            .map(|line| {
                // The column has the background; its lines sit on it, so a
                // gradient runs unbroken behind them.
                Widget::text(self.element, line.clone())
                    .with_size(SizePolicy::Fixed(self.line_height))
                    .without_background()
            })
            .collect();
        Widget::column(self.element, rows)
    }

    fn state(&self) -> WidgetState {
        WidgetState::Focused
    }
}

impl Tooltip {
    /// How far above the pointer the tooltip's bottom is.
    const GAP: i32 = 10;

    /// Notes that the pointer rests on what has the tooltip `text` (none
    /// for nothing), which shows after `delay` (none: never). Returns
    /// whether a tooltip shown went away, so the output is drawn again.
    pub(crate) fn follow(
        &mut self,
        text: Option<String>,
        delay: Option<Duration>,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) -> bool {
        if text == self.wanted {
            return false;
        }
        self.wanted = text;
        self.timer = None;
        let was_shown = self.shown.take().is_some();
        if let (Some(_), Some(delay)) = (&self.wanted, delay) {
            let (handle, ledger) = event_loop;
            match Timer::once(handle, ledger, delay, Compositor::show_tooltip) {
                Ok(timer) => self.timer = Some(timer),
                Err(error) => tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "a tooltip cannot wait to show"
                ),
            }
        }
        was_shown
    }

    /// Hides the tooltip until the pointer rests on something else;
    /// returns whether one was shown.
    pub(crate) fn hide(&mut self) -> bool {
        self.timer = None;
        self.shown.take().is_some()
    }

    /// Shows the tooltip wanted, by the pointer at `pointer`, inside
    /// `bounds`, as `style` styles tooltips.
    pub(crate) fn show(
        &mut self,
        pointer: Point<i32, Logical>,
        bounds: Area,
        style: (&Style, &mut Typesetter),
    ) {
        self.timer = None;
        let Some(text) = &self.wanted else {
            return;
        };
        let (style, typesetter) = style;
        let tooltip = style.element_style(StyleElement::Tooltip, WidgetState::Focused);
        let line_height = typesetter.line_height(&tooltip.text.font);
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let widest = lines
            .iter()
            .map(|line| typesetter.text_width(line, &tooltip.text.font))
            .max()
            .unwrap_or(0);
        let count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        // Each line is drawn inside the style's padding, so a row is a
        // line and the padding above and below it.
        let row_height = line_height + tooltip.padding.vertical();
        let width = widest + tooltip.padding.horizontal();
        let height = count.saturating_mul(row_height);
        let border = i32::try_from(tooltip.border.width).unwrap_or(0);
        let outer_width = to_i32(width) + 2 * border;
        let outer_height = to_i32(height) + 2 * border;
        let mut x = pointer.x - outer_width / 2;
        let mut y = pointer.y - Self::GAP - outer_height;
        if y < bounds.loc.y {
            y = pointer.y + 2 * Self::GAP;
        }
        x = x.clamp(
            bounds.loc.x,
            (bounds.loc.x + bounds.size.w - outer_width).max(bounds.loc.x),
        );
        y = y.clamp(
            bounds.loc.y,
            (bounds.loc.y + bounds.size.h - outer_height).max(bounds.loc.y),
        );
        let area = Rectangle::new((x, y).into(), (outer_width, outer_height).into());
        let content = Lines {
            texts: lines,
            line_height: row_height,
            element: StyleElement::Tooltip,
        };
        let corners = tooltip.corner_radii();
        let mut panel = Panel::new(content, vaht_ui::Size::new(width, height));
        panel.set_corners(corners.inset(tooltip.border.width));
        panel.paint(style, typesetter);
        let mut texture = PanelTexture::new();
        texture.upload(&panel);
        let mut outline = Outline::default();
        outline.update_as(Some(area), style, StyleElement::Tooltip, corners);
        self.shown = Some(Shown {
            area,
            border,
            texture,
            outline,
            opacity: tooltip.opacity(),
        });
    }

    /// Returns the elements that draw the tooltip, topmost first; none
    /// while none shows.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: SceneRenderer,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(shown) = &self.shown else {
            return Vec::new();
        };
        let mut elements = shown
            .outline
            .render_elements(renderer, scale, shown.opacity);
        let inside = shown.area.loc + Point::from((shown.border, shown.border));
        elements.extend(
            shown
                .texture
                .render_element(renderer, inside, scale, shown.opacity),
        );
        elements
    }
}

/// Converts a pixel extent to a coordinate.
fn to_i32(extent: u32) -> i32 {
    i32::try_from(extent).unwrap_or(i32::MAX)
}
