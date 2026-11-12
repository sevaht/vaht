//! Painting a laid-out panel: backgrounds, bevels, text and button pictures.
//!
//! This is the only code that turns styles into pixels; every panel goes
//! through it.

use tiny_skia::{
    GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Rect as SkiaRect, Shader, SpreadMode,
    Stroke, Transform,
};

use crate::color::Color;
use crate::geometry::Rect;
use crate::layout::{Content, Layout};
use crate::text::Typesetter;
use crate::texture::{Bevel, Fill, GradientDirection, Texture};
use crate::theme::{Theme, WidgetState};
use crate::widget::Glyph;

/// The opacity of a bevel's light and dark edges: enough to read as relief
/// on any fill without hiding it.
const BEVEL_ALPHA: u8 = 96;

/// A button picture's stroke width relative to its box: thin enough to stay
/// crisp at titlebar sizes.
const GLYPH_STROKE_WIDTH: f32 = 1.5;

/// Paints every widget of `layout` into `pixmap`, styled by `theme` in the
/// state `state_of` gives for each widget's hit.
pub(crate) fn paint<Hit: Copy>(
    pixmap: &mut Pixmap,
    layout: &Layout<Hit>,
    theme: &Theme,
    typesetter: &mut Typesetter,
    state_of: impl Fn(Option<Hit>) -> WidgetState,
) {
    for placed in layout.placed() {
        let style = theme.style(placed.element, state_of(placed.hit));
        fill_texture(pixmap, style.texture, placed.rect);
        let inner = placed.rect.inset(style.padding);
        match &placed.content {
            Content::Text(text) => typesetter.draw(pixmap, text, &style.text, inner),
            Content::Glyph(glyph) => draw_glyph(pixmap, *glyph, style.glyph_color, inner),
            Content::Background => {}
        }
    }
}

/// Fills `rect` with `texture`'s fill, then draws its bevel.
fn fill_texture(pixmap: &mut Pixmap, texture: Texture, rect: Rect) {
    let Some(area) = to_skia(rect) else {
        return;
    };
    let shader = match texture.fill {
        Fill::Solid(color) => Shader::SolidColor(color.to_skia()),
        Fill::Gradient {
            direction,
            from,
            to,
        } => {
            let (start, end) = match direction {
                GradientDirection::Vertical => {
                    ((area.left(), area.top()), (area.left(), area.bottom()))
                }
                GradientDirection::Horizontal => {
                    ((area.left(), area.top()), (area.right(), area.top()))
                }
                GradientDirection::Diagonal => {
                    ((area.left(), area.top()), (area.right(), area.bottom()))
                }
                GradientDirection::CrossDiagonal => {
                    ((area.right(), area.top()), (area.left(), area.bottom()))
                }
            };
            LinearGradient::new(
                start.into(),
                end.into(),
                vec![GradientStop::new(0.0, from.to_skia()), GradientStop::new(1.0, to.to_skia())],
                SpreadMode::Pad,
                Transform::identity(),
            )
            // A gradient whose ends coincide (a one-pixel area) is one color.
            .unwrap_or(Shader::SolidColor(from.to_skia()))
        }
    };
    let paint = Paint {
        shader,
        ..Paint::default()
    };
    pixmap.fill_rect(area, &paint, Transform::identity(), None);
    draw_bevel(pixmap, texture.bevel, rect);
}

/// Draws one-pixel light and dark edges that make `rect` look raised or
/// sunken.
fn draw_bevel(pixmap: &mut Pixmap, bevel: Bevel, rect: Rect) {
    let light = Color::rgba(255, 255, 255, BEVEL_ALPHA);
    let dark = Color::rgba(0, 0, 0, BEVEL_ALPHA);
    let (top_left, bottom_right) = match bevel {
        Bevel::Flat => return,
        Bevel::Raised => (light, dark),
        Bevel::Sunken => (dark, light),
    };
    if rect.width < 2 || rect.height < 2 {
        return;
    }
    let right = rect.x + i32::try_from(rect.width - 1).expect("panel widths fit in i32");
    let bottom = rect.y + i32::try_from(rect.height - 1).expect("panel heights fit in i32");
    let edges = [
        (Rect::new(rect.x, rect.y, rect.width, 1), top_left),
        (Rect::new(rect.x, rect.y, 1, rect.height), top_left),
        (Rect::new(rect.x, bottom, rect.width, 1), bottom_right),
        (Rect::new(right, rect.y, 1, rect.height), bottom_right),
    ];
    for (edge, color) in edges {
        if let Some(edge) = to_skia(edge) {
            let mut paint = Paint::default();
            paint.set_color(color.to_skia());
            pixmap.fill_rect(edge, &paint, Transform::identity(), None);
        }
    }
}

/// Strokes `glyph` in `color`, centered in the largest square inside `rect`.
fn draw_glyph(pixmap: &mut Pixmap, glyph: Glyph, color: Color, rect: Rect) {
    let side = rect.width.min(rect.height);
    if side == 0 {
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "panel coordinates are far below 2^24"
    )]
    let (left, top, side) = (
        rect.x as f32 + (rect.width - side) as f32 / 2.0,
        rect.y as f32 + (rect.height - side) as f32 / 2.0,
        side as f32,
    );
    let (right, bottom) = (left + side, top + side);
    let mut path = PathBuilder::new();
    match glyph {
        Glyph::Close => {
            path.move_to(left, top);
            path.line_to(right, bottom);
            path.move_to(right, top);
            path.line_to(left, bottom);
        }
        Glyph::Maximize => {
            path.push_rect(
                SkiaRect::from_ltrb(left, top, right, bottom).expect("a square of positive side"),
            );
            // Fluxbox's maximize box has a heavier top edge, like a titlebar.
            path.move_to(left, top + 1.0);
            path.line_to(right, top + 1.0);
        }
        Glyph::Minimize => {
            path.move_to(left, bottom);
            path.line_to(right, bottom);
        }
        Glyph::Shade => {
            path.move_to(left, top);
            path.line_to(right, top);
        }
        Glyph::Stick => {
            let middle = side / 4.0;
            path.push_rect(
                SkiaRect::from_xywh(left + middle, top + middle, side / 2.0, side / 2.0)
                    .expect("a square of positive side"),
            );
        }
    }
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color(color.to_skia());
        paint.anti_alias = true;
        let stroke = Stroke {
            width: GLYPH_STROKE_WIDTH,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// Converts a rectangle for tiny-skia; `None` when it covers no pixels.
fn to_skia(rect: Rect) -> Option<SkiaRect> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "panel coordinates are far below 2^24"
    )]
    let rect = SkiaRect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    rect
}
