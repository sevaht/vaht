//! Painting a laid-out panel: backgrounds, bevels, text and button pictures.
//!
//! This is the only code that turns styles into pixels; every panel goes
//! through it.

use tiny_skia::{
    ColorU8, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, Rect as SkiaRect,
    SpreadMode, Stroke, Transform,
};

use crate::color::Color;
use crate::geometry::Rect;
use crate::icon::WidgetIcon;
use crate::image::Image;
use crate::layout::{Content, Layout, Placed};
use crate::style::{Style, WidgetState};
use crate::text::{TextStyle, Typesetter};
use crate::texture::render::{self, GradientImage};
use crate::texture::{Bevel, Fill, ImageFit, Texture};
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

    /// Paints every widget of `layout`, styled by `style` in the state
    /// `state_of` gives for each placed widget, and whether the pointer is
    /// on it.
    pub(crate) fn paint<Hit: Copy>(
        &mut self,
        layout: &Layout<Hit>,
        style: &Style,
        typesetter: &mut Typesetter,
        state_of: impl Fn(&Placed<Hit>) -> (WidgetState, bool),
    ) {
        for placed in layout.placed() {
            let (state, is_hovered) = state_of(placed);
            let style = style.element_style_under_pointer(placed.element, state, is_hovered);
            if placed.has_background {
                self.fill_texture(&style.texture, placed.rect);
            }
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
                match style.glyph_images.get(mark) {
                    // The style's own picture, at its own size.
                    Some(image) => self.draw_centered(image, square),
                    None => self.draw_glyph(mark, color, square),
                }
            }
            match &placed.content {
                Content::Text(text) => typesetter.draw(self.pixmap, text, &style.text, inner),
                Content::Glyph(glyph) => {
                    // A style may keep the maximize button as it is while the
                    // window is maximized.
                    let glyph = if *glyph == Glyph::Restore && !style.restore_glyph {
                        Glyph::Maximize
                    } else {
                        *glyph
                    };
                    // A style of images that has none for restore keeps the
                    // maximize one there.
                    let image = style.glyph_images.get(glyph).or_else(|| {
                        (glyph == Glyph::Restore)
                            .then(|| style.glyph_images.get(Glyph::Maximize))
                            .flatten()
                    });
                    match image {
                        // A picture of the style's own is the whole button.
                        Some(image) => self.draw_stretched(image, placed.rect),
                        None => self.draw_glyph(glyph, style.glyph_color, inner),
                    }
                }
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
    fn fill_texture(&mut self, texture: &Texture, rect: Rect) {
        match &texture.fill {
            Fill::Parent => {}
            Fill::Image {
                image,
                fit: ImageFit::Stretch,
            } => self.draw_stretched(image, rect),
            Fill::Image {
                image,
                fit: ImageFit::Tile,
            } => self.draw_tiled(image, rect),
            &Fill::Solid(color) => {
                if let Some(area) = rect.to_skia() {
                    let mut paint = Paint::default();
                    paint.set_color(color.to_skia());
                    self.pixmap
                        .fill_rect(area, &paint, Transform::identity(), None);
                }
                self.draw_solid_bevel(texture.bevel, color, rect);
            }
            &Fill::Gradient {
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

    /// Draws `picture` stretched to fill `rect`, smoothly.
    #[expect(
        clippy::cast_precision_loss,
        reason = "pictures are at most a few thousand pixels across, and so are panels"
    )]
    fn draw_stretched(&mut self, picture: &Image, rect: Rect) {
        let (Some(source), Some(area)) = (Self::pixmap_of(picture), rect.to_skia()) else {
            return;
        };
        let scale_x = rect.width as f32 / picture.width() as f32;
        let scale_y = rect.height as f32 / picture.height() as f32;
        let paint = Paint {
            shader: Pattern::new(
                source.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                Transform::from_scale(scale_x, scale_y)
                    .post_translate(rect.x as f32, rect.y as f32),
            ),
            ..Paint::default()
        };
        self.pixmap
            .fill_rect(area, &paint, Transform::identity(), None);
    }

    /// Draws `picture` at its own size, in the middle of `rect`.
    fn draw_centered(&mut self, picture: &Image, rect: Rect) {
        let Some(source) = Self::pixmap_of(picture) else {
            return;
        };
        let offset = |space: u32, own: u32| {
            i32::try_from(space).unwrap_or(0) / 2 - i32::try_from(own).unwrap_or(0) / 2
        };
        self.pixmap.draw_pixmap(
            rect.x + offset(rect.width, picture.width()),
            rect.y + offset(rect.height, picture.height()),
            source.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draws `picture` at its own size, repeated across `rect` from its top
    /// left corner.
    #[expect(
        clippy::cast_precision_loss,
        reason = "panel coordinates are far below 2^24"
    )]
    fn draw_tiled(&mut self, picture: &Image, rect: Rect) {
        let (Some(source), Some(area)) = (Self::pixmap_of(picture), rect.to_skia()) else {
            return;
        };
        let paint = Paint {
            shader: Pattern::new(
                source.as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Nearest,
                1.0,
                Transform::from_translate(rect.x as f32, rect.y as f32),
            ),
            ..Paint::default()
        };
        self.pixmap
            .fill_rect(area, &paint, Transform::identity(), None);
    }

    /// Returns `picture` as pixels to paint with; none if it is empty.
    fn pixmap_of(picture: &Image) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(picture.width(), picture.height())?;
        for (target, color) in pixmap.pixels_mut().iter_mut().zip(picture.pixels()) {
            *target =
                ColorU8::from_rgba(color.red, color.green, color.blue, color.alpha).premultiply();
        }
        Some(pixmap)
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
        // Fluxbox's toolbar arrow (FbDrawable::drawTriangle at its default
        // toolbar.button.scale, 250): as wide as tall, two fifths of the
        // button, cut to an even number of pixels, centered; 8 in 20, as
        // measured on Fluxbox itself.
        let arrow = (side * 100.0 / 250.0 / 2.0).floor() * 2.0;
        let (tip, base, half_height) = match glyph {
            Glyph::ArrowLeft => (middle_x - arrow / 2.0, middle_x + arrow / 2.0, arrow / 2.0),
            Glyph::ArrowRight => (middle_x + arrow / 2.0, middle_x - arrow / 2.0, arrow / 2.0),
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
            | Glyph::Restore
            | Glyph::Minimize
            | Glyph::Shade
            | Glyph::Stick
            | Glyph::Stuck
            | Glyph::Menu
            | Glyph::Submenu
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

    /// Returns the lines that stroke `glyph` in the square of `side` at
    /// `left`, `top`; empty for a glyph that is filled instead.
    fn strokes(glyph: Glyph, left: f32, top: f32, side: f32) -> PathBuilder {
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
                    SkiaRect::from_ltrb(left, top, right, bottom)
                        .expect("a square of positive side"),
                );
                // Fluxbox's maximize box has a heavier top edge, like a titlebar.
                path.move_to(left, top + 1.0);
                path.line_to(right, top + 1.0);
            }
            Glyph::Restore => {
                // Two boxes of the same size and the same lines, the one
                // behind up and to the right: the front box whole, the back one
                // only where it shows. Their top edges are plain (no heavier
                // second line, as the maximize box has), so the first row of
                // each box's inside stays clean pixels, not a blend of its
                // borders.
                let offset = (side * 0.3).round().max(2.0);
                path.push_rect(
                    SkiaRect::from_ltrb(left, top + offset, right - offset, bottom)
                        .expect("a box of positive size"),
                );
                path.move_to(left + offset, top + offset);
                path.line_to(left + offset, top);
                path.line_to(right, top);
                path.line_to(right, bottom - offset);
                path.line_to(right - offset, bottom - offset);
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
            Glyph::Stuck => {
                // The square of the stick button, crossed: it is pinned.
                let middle = side / 4.0;
                let (near, far) = (middle, side - middle);
                path.push_rect(
                    SkiaRect::from_xywh(left + middle, top + middle, side / 2.0, side / 2.0)
                        .expect("a square of positive side"),
                );
                path.move_to(left + near, top + near);
                path.line_to(left + far, top + far);
                path.move_to(left + far, top + near);
                path.line_to(left + near, top + far);
            }
            Glyph::Menu => {
                for row in [0.0, 0.5, 1.0] {
                    path.move_to(left, top + side * row);
                    path.line_to(right, top + side * row);
                }
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
            Glyph::Submenu
            | Glyph::ArrowLeft
            | Glyph::ArrowRight
            | Glyph::Check
            | Glyph::BulletRight
            | Glyph::BulletLeft
            | Glyph::BulletDiamond => {}
        }
        path
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
        let path = Self::strokes(glyph, left, top, side);
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
