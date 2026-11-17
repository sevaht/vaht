//! Painting a laid-out panel: backgrounds, bevels, text and button pictures.
//!
//! This is the only code that turns styles into pixels; every panel goes
//! through it.

use tiny_skia::{
    ColorU8, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkiaRect, Stroke, Transform,
};

use crate::color::Color;
use crate::geometry::Rect;
use crate::icon::WidgetIcon;
use crate::layout::{Content, Layout, Placed};
use crate::text::{TextStyle, Typesetter};
use crate::texture::render::{self, GradientImage};
use crate::texture::{Bevel, Fill, Texture};
use crate::theme::{Theme, WidgetState};
use crate::widget::{Glyph, MarkSide, TextField};

/// A button picture's stroke width relative to its box: thin enough to stay
/// crisp at titlebar sizes.
const GLYPH_STROKE_WIDTH: f32 = 1.5;

/// A pixmap being painted: the one place styles become pixels.
#[derive(Debug)]
pub(crate) struct Canvas<'pixmap> {
    /// The pixels painted into.
    pixmap: &'pixmap mut Pixmap,
}

impl<'pixmap> Canvas<'pixmap> {
    /// Paints into `pixmap`.
    pub(crate) const fn new(pixmap: &'pixmap mut Pixmap) -> Self {
        Self { pixmap }
    }

    /// Paints every widget of `layout`, styled by `theme` in the state
    /// `state_of` gives for each placed widget.
    pub(crate) fn paint<Hit: Copy>(
        &mut self,
        layout: &Layout<Hit>,
        theme: &Theme,
        typesetter: &mut Typesetter,
        state_of: impl Fn(&Placed<Hit>) -> WidgetState,
    ) {
        for placed in layout.placed() {
            let style = theme.style(placed.element, state_of(placed));
            self.fill_texture(style.texture, placed.rect);
            let mut inner = placed.rect.inset(style.padding);
            if let Some(icon) = &placed.icon {
                self.draw_icon(icon, WidgetIcon::square_in(placed.rect));
                inner = inner.without_left(WidgetIcon::span(placed.rect.height));
            }
            if let Some(mark) = placed.mark {
                let side = placed.rect.height.min(placed.rect.width);
                let offset = match placed.mark_side {
                    MarkSide::Left => 0,
                    MarkSide::Right => {
                        i32::try_from(placed.rect.width - side).expect("panel widths fit in i32")
                    }
                };
                let square = Rect::new(placed.rect.x + offset, placed.rect.y, side, side);
                // A chosen item's check is the glyph color; a bullet is the
                // text's, as Fluxbox draws them.
                let color = if mark == Glyph::Check {
                    style.glyph_color
                } else {
                    style.text.color
                };
                self.draw_glyph(mark, color, square);
            }
            match &placed.content {
                Content::Text(text) => typesetter.draw(self.pixmap, text, &style.text, inner),
                Content::Glyph(glyph) => self.draw_glyph(*glyph, style.glyph_color, inner),
                Content::Field(field) => self.draw_field(field, &style.text, inner, typesetter),
                Content::Background => {}
            }
        }
    }

    /// Draws `field` in `area` (see [`Typesetter::draw_field`]) and its
    /// cursor: a line in the text's color, a pixel in from the area's top
    /// and bottom.
    fn draw_field(
        &mut self,
        field: &TextField,
        style: &TextStyle,
        area: Rect,
        typesetter: &mut Typesetter,
    ) {
        let cursor = typesetter.draw_field(self.pixmap, field, style, area);
        let x = area.x + i32::try_from(cursor).expect("panel widths fit in i32");
        let line = Rect::new(x, area.y + 1, 1, area.height.saturating_sub(2));
        if let Some(line) = line.to_skia() {
            let mut paint = Paint::default();
            paint.set_color(style.color.to_skia());
            self.pixmap
                .fill_rect(line, &paint, Transform::identity(), None);
        }
    }

    /// Draws `icon` into `square`, resized to it beforehand
    /// ([`crate::Icon::pixels_at`]) rather than scaled as it is drawn.
    fn draw_icon(&mut self, icon: &WidgetIcon, square: Rect) {
        let Some(pixels) = icon.icon.pixels_at(square.width) else {
            return;
        };
        let paint = PixmapPaint {
            opacity: icon.emphasis.opacity(),
            ..PixmapPaint::default()
        };
        self.pixmap.draw_pixmap(
            square.x,
            square.y,
            pixels.as_ref().as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
    }

    /// Fills `rect` with `texture`'s fill and bevel, as Fluxbox renders
    /// textures ([`crate::texture::render`]).
    fn fill_texture(&mut self, texture: Texture, rect: Rect) {
        match texture.fill {
            Fill::Parent => {}
            Fill::Solid(color) => {
                if let Some(area) = rect.to_skia() {
                    let mut paint = Paint::default();
                    paint.set_color(color.to_skia());
                    self.pixmap
                        .fill_rect(area, &paint, Transform::identity(), None);
                }
                self.draw_solid_bevel(texture.bevel, color, rect);
            }
            Fill::Gradient {
                direction,
                from,
                to,
            } => {
                if rect.width == 0 || rect.height == 0 {
                    return;
                }
                let image = GradientImage::render(
                    direction,
                    (from, to),
                    texture.bevel,
                    (rect.width, rect.height),
                );
                self.draw_image(&image, rect);
            }
        }
    }

    /// Draws `image` at `rect`'s top left corner.
    fn draw_image(&mut self, image: &GradientImage, rect: Rect) {
        let Some(mut pixmap) = Pixmap::new(image.width, image.height) else {
            return;
        };
        for (target, color) in pixmap.pixels_mut().iter_mut().zip(&image.pixels) {
            *target =
                ColorU8::from_rgba(color.red, color.green, color.blue, color.alpha).premultiply();
        }
        self.pixmap.draw_pixmap(
            rect.x,
            rect.y,
            pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draws a solid area's bevel: its edges in `color` made lighter and
    /// darker, as Fluxbox's `drawBevelRectangle` draws them, the bottom and
    /// right first, then the top and left over their corners.
    fn draw_solid_bevel(&mut self, bevel: Bevel, color: Color, rect: Rect) {
        let (light, dark) = (render::lightened(color), render::darkened(color));
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
            (Rect::new(rect.x, bottom, rect.width, 1), bottom_right),
            (Rect::new(right, rect.y, 1, rect.height), bottom_right),
            (Rect::new(rect.x, rect.y, rect.width, 1), top_left),
            (Rect::new(rect.x, rect.y, 1, rect.height), top_left),
        ];
        for (edge, color) in edges {
            if let Some(edge) = edge.to_skia() {
                let mut paint = Paint::default();
                paint.set_color(color.to_skia());
                self.pixmap
                    .fill_rect(edge, &paint, Transform::identity(), None);
            }
        }
    }

    /// Returns the filled shape of `glyph` in the square of `side` at
    /// `left`, `top`: an arrow's triangle, half as wide as tall, pointing its
    /// way, or a check's square, half as wide as the square, as Fluxbox marks
    /// a chosen menu item. `None` for a glyph that is stroked.
    fn filled(glyph: Glyph, left: f32, top: f32, side: f32) -> Option<tiny_skia::Path> {
        // Fluxbox's bullet triangle is a third of the square, centered.
        let third = side / 3.0;
        let (middle_x, middle_y) = (left + side / 2.0, top + side / 2.0);
        let (tip, base, half_height) = match glyph {
            Glyph::ArrowLeft => (left + side / 4.0, left + side * 3.0 / 4.0, side / 2.0),
            Glyph::ArrowRight => (left + side * 3.0 / 4.0, left + side / 4.0, side / 2.0),
            Glyph::BulletRight => (middle_x + third / 2.0, middle_x - third / 2.0, third / 2.0),
            Glyph::BulletLeft => (middle_x - third / 2.0, middle_x + third / 2.0, third / 2.0),
            Glyph::BulletDiamond => {
                let mut path = PathBuilder::new();
                path.move_to(middle_x - 3.0, middle_y);
                path.line_to(middle_x, middle_y - 3.0);
                path.line_to(middle_x + 3.0, middle_y);
                path.line_to(middle_x, middle_y + 3.0);
                path.close();
                return path.finish();
            }
            Glyph::Check => {
                let quarter = side / 4.0;
                return Some(PathBuilder::from_rect(SkiaRect::from_xywh(
                    left + quarter,
                    top + quarter,
                    side / 2.0,
                    side / 2.0,
                )?));
            }
            Glyph::Close
            | Glyph::Maximize
            | Glyph::Minimize
            | Glyph::Shade
            | Glyph::Stick
            | Glyph::BulletSquare => {
                return None;
            }
        };
        let mut path = PathBuilder::new();
        path.move_to(tip, middle_y);
        path.line_to(base, middle_y - half_height);
        path.line_to(base, middle_y + half_height);
        path.close();
        path.finish()
    }

    /// Strokes `glyph` in `color`, centered in the largest square inside `rect`.
    fn draw_glyph(&mut self, glyph: Glyph, color: Color, rect: Rect) {
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
        if let Some(arrow) = Self::filled(glyph, left, top, side) {
            let mut paint = Paint::default();
            paint.set_color(color.to_skia());
            paint.anti_alias = true;
            self.pixmap.fill_path(
                &arrow,
                &paint,
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
            return;
        }
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
                    SkiaRect::from_ltrb(left, top, right, bottom)
                        .expect("a square of positive side"),
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
            Glyph::BulletSquare => {
                // A one-pixel outline, half the square, as Fluxbox's.
                let quarter = (side / 4.0).floor() + 0.5;
                if let Some(square) =
                    SkiaRect::from_xywh(left + quarter, top + quarter, side / 2.0, side / 2.0)
                {
                    path.push_rect(square);
                }
            }
            Glyph::ArrowLeft
            | Glyph::ArrowRight
            | Glyph::Check
            | Glyph::BulletRight
            | Glyph::BulletLeft
            | Glyph::BulletDiamond => {}
        }
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color(color.to_skia());
            paint.anti_alias = true;
            let stroke = Stroke {
                width: GLYPH_STROKE_WIDTH,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}
