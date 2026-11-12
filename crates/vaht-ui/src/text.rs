//! Shaping, measuring and drawing text, with Fluxbox's halo and shadow.
//!
//! A [`Typesetter`] owns cosmic-text's font database and glyph cache. Loading
//! the system's fonts takes tens of milliseconds, so there is one typesetter,
//! lent to whatever is being drawn. Text is drawn into a layer the size of its
//! area, which clips it there, and the layer is then composited.

use std::fmt;

use cosmic_text::{
    Attrs, Buffer, Command, Family, FontSystem, Metrics, Shaping, Style as SlantStyle, SwashCache,
    Weight, Wrap,
};
use tiny_skia::{Paint, PathBuilder, Pixmap, PixmapPaint, PremultipliedColorU8, Stroke, Transform};

use crate::color::Color;
use crate::geometry::Rect;

/// A typeface at a size.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Font {
    /// The family name, as fontconfig knows it (`DejaVu Sans`).
    pub family: String,
    /// The size in points; pixels depend on the typesetter's resolution.
    pub size_points: u16,
    /// How heavy the strokes are.
    pub weight: FontWeight,
    /// Whether the letters slant.
    pub is_italic: bool,
}

/// How heavy a font's strokes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontWeight {
    /// Regular text.
    #[default]
    Normal,
    /// Heavier strokes.
    Bold,
}

/// An effect drawn under text to make it readable on any background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextEffect {
    /// Nothing.
    #[default]
    None,
    /// An outline of `color` around every glyph.
    Halo(Color),
    /// A copy of the text in `color`, offset down and right.
    Shadow {
        /// The shadow's color.
        color: Color,
        /// How far right the shadow lies, in pixels.
        offset_x: i8,
        /// How far down the shadow lies, in pixels.
        offset_y: i8,
    },
}

/// Where text sits horizontally in its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Justify {
    /// Against the left edge.
    #[default]
    Left,
    /// Centered.
    Center,
    /// Against the right edge.
    Right,
}

impl Justify {
    /// Returns how far from the left a line of `line_width` starts in an
    /// area `area_width` wide; never negative, so long text stays readable
    /// from its start.
    fn offset(self, line_width: f32, area_width: u32) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "panel widths are far below 2^24"
        )]
        let free = (area_width as f32 - line_width).max(0.0);
        match self {
            Self::Left => 0.0,
            Self::Center => (free / 2.0).round(),
            Self::Right => free,
        }
    }
}

/// How a piece of text looks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextStyle {
    /// The typeface.
    pub font: Font,
    /// The color of the letters.
    pub color: Color,
    /// What is drawn under them.
    pub effect: TextEffect,
    /// Where they sit horizontally.
    pub justify: Justify,
}

/// Shapes, measures and draws text; there is one, lent where text is drawn.
pub struct Typesetter {
    /// Every font on the system, and shaping state.
    font_system: FontSystem,
    /// Rasterized glyphs and their outlines.
    cache: SwashCache,
    /// The screen resolution that turns points into pixels.
    dots_per_inch: u16,
}

impl Typesetter {
    /// The resolution assumed when none is configured; also Fluxbox's.
    pub const DEFAULT_DOTS_PER_INCH: u16 = 96;

    /// Points in an inch: a point is 1/72 inch.
    const POINTS_PER_INCH: f32 = 72.0;

    /// The width of a halo's outline, in pixels; fluxland draws the same.
    const HALO_WIDTH: f32 = 2.0;

    /// Loads the system's fonts, for a screen of `dots_per_inch`.
    #[must_use]
    pub fn new(dots_per_inch: u16) -> Self {
        Self {
            font_system: FontSystem::new(),
            cache: SwashCache::new(),
            dots_per_inch,
        }
    }

    /// Returns how wide `text` is in `font`, in whole pixels, rounded up.
    pub fn text_width(&mut self, text: &str, font: &Font) -> u32 {
        let pixel_size = self.pixel_size(font);
        let buffer = self.lay_out(text, font, pixel_size);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a line's width is positive and far below u32::MAX"
        )]
        let pixels = width.ceil() as u32;
        pixels
    }

    /// Returns the height of a line of `font`, in whole pixels, rounded up.
    #[must_use]
    pub fn line_height(&self, font: &Font) -> u32 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "font sizes are small and positive"
        )]
        let pixels = self.pixel_size(font).ceil() as u32;
        pixels
    }

    /// Draws `text` in `style` into `area` of `target`, vertically centered
    /// and clipped to the area.
    pub fn draw(&mut self, target: &mut Pixmap, text: &str, style: &TextStyle, area: Rect) {
        if text.is_empty() {
            return;
        }
        let Some(mut layer) = Pixmap::new(area.width, area.height) else {
            return;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "panel heights are far below 2^24"
        )]
        let line_height = area.height as f32;
        let mut buffer = self.lay_out(text, &style.font, line_height);
        let Some(line_width) = buffer.layout_runs().next().map(|line| line.line_w) else {
            return;
        };
        let offset_x = style.justify.offset(line_width, area.width);
        match style.effect {
            TextEffect::None => {}
            TextEffect::Halo(color) => self.stroke_outlines(&mut layer, &buffer, offset_x, color),
            TextEffect::Shadow {
                color,
                offset_x: shadow_x,
                offset_y: shadow_y,
            } => self.fill_glyphs(
                &mut layer,
                &mut buffer,
                (offset_x + f32::from(shadow_x), f32::from(shadow_y)),
                color,
            ),
        }
        self.fill_glyphs(&mut layer, &mut buffer, (offset_x, 0.0), style.color);
        target.draw_pixmap(
            area.x,
            area.y,
            layer.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Returns `font`'s size in pixels at this resolution.
    fn pixel_size(&self, font: &Font) -> f32 {
        f32::from(font.size_points) * f32::from(self.dots_per_inch) / Self::POINTS_PER_INCH
    }

    /// Shapes `text` as one unwrapped line `line_height` tall.
    fn lay_out(&mut self, text: &str, font: &Font, line_height: f32) -> Buffer {
        let metrics = Metrics::new(self.pixel_size(font), line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(Wrap::None);
        let weight = match font.weight {
            FontWeight::Normal => Weight::NORMAL,
            FontWeight::Bold => Weight::BOLD,
        };
        let slant = if font.is_italic {
            SlantStyle::Italic
        } else {
            SlantStyle::Normal
        };
        let attributes = Attrs::new()
            .family(Family::Name(&font.family))
            .weight(weight)
            .style(slant);
        buffer.set_text(text, &attributes, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    /// Fills the glyphs' rasterized coverage in `color`, moved by `offset`.
    fn fill_glyphs(
        &mut self,
        layer: &mut Pixmap,
        buffer: &mut Buffer,
        offset: (f32, f32),
        color: Color,
    ) {
        let (width, height) = (layer.width(), layer.height());
        let pixels = layer.pixels_mut();
        let text_color = cosmic_text::Color::rgba(color.red, color.green, color.blue, color.alpha);
        #[expect(clippy::cast_possible_truncation, reason = "offsets are a few pixels")]
        let (shift_x, shift_y) = (offset.0.round() as i32, offset.1.round() as i32);
        buffer.draw(
            &mut self.font_system,
            &mut self.cache,
            text_color,
            |x, y, width_pixels, height_pixels, coverage| {
                for row in y + shift_y..y + shift_y + to_i32(height_pixels) {
                    for column in x + shift_x..x + shift_x + to_i32(width_pixels) {
                        if let (Ok(column), Ok(row)) = (u32::try_from(column), u32::try_from(row))
                            && column < width
                            && row < height
                        {
                            let index = usize::try_from(row * width + column)
                                .expect("a pixel index fits in usize");
                            pixels[index] = blend_over(pixels[index], coverage);
                        }
                    }
                }
            },
        );
    }

    /// Strokes every glyph's outline in `color`: Fluxbox's halo.
    fn stroke_outlines(
        &mut self,
        layer: &mut Pixmap,
        buffer: &Buffer,
        offset_x: f32,
        color: Color,
    ) {
        let mut path = PathBuilder::new();
        for line in buffer.layout_runs() {
            for glyph in line.glyphs {
                let physical = glyph.physical((offset_x, 0.0), 1.0);
                let Some(commands) = self
                    .cache
                    .get_outline_commands(&mut self.font_system, physical.cache_key)
                else {
                    continue;
                };
                #[expect(clippy::cast_precision_loss, reason = "glyph positions are small")]
                let (origin_x, origin_y) = (physical.x as f32, line.line_y + physical.y as f32);
                // Outlines have y pointing up; the layer's points down.
                let at = |x: f32, y: f32| (origin_x + x, origin_y - y);
                for command in commands {
                    match *command {
                        Command::MoveTo(point) => {
                            let (x, y) = at(point.x, point.y);
                            path.move_to(x, y);
                        }
                        Command::LineTo(point) => {
                            let (x, y) = at(point.x, point.y);
                            path.line_to(x, y);
                        }
                        Command::QuadTo(control, point) => {
                            let ((control_x, control_y), (x, y)) =
                                (at(control.x, control.y), at(point.x, point.y));
                            path.quad_to(control_x, control_y, x, y);
                        }
                        Command::CurveTo(first, second, point) => {
                            let ((first_x, first_y), (second_x, second_y), (x, y)) = (
                                at(first.x, first.y),
                                at(second.x, second.y),
                                at(point.x, point.y),
                            );
                            path.cubic_to(first_x, first_y, second_x, second_y, x, y);
                        }
                        Command::Close => path.close(),
                    }
                }
            }
        }
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color(color.to_skia());
            paint.anti_alias = true;
            let stroke = Stroke {
                width: Self::HALO_WIDTH,
                ..Stroke::default()
            };
            layer.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}

impl fmt::Debug for Typesetter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Typesetter")
            .field("dots_per_inch", &self.dots_per_inch)
            .finish_non_exhaustive()
    }
}

/// Converts a glyph's pixel extent to a signed coordinate.
fn to_i32(extent: u32) -> i32 {
    i32::try_from(extent).expect("glyph extents are a few dozen pixels")
}

/// Composites `source` (straight alpha, as cosmic-text gives coverage) over
/// a premultiplied `destination` pixel.
fn blend_over(
    destination: PremultipliedColorU8,
    source: cosmic_text::Color,
) -> PremultipliedColorU8 {
    let alpha = u16::from(source.a());
    let keep = 255 - alpha;
    let channel = |source: u8, destination: u8| {
        let premultiplied = u16::from(source) * alpha / 255;
        u8::try_from(premultiplied + u16::from(destination) * keep / 255)
            .expect("a source-over blend stays within a byte")
    };
    PremultipliedColorU8::from_rgba(
        channel(source.r(), destination.red()),
        channel(source.g(), destination.green()),
        channel(source.b(), destination.blue()),
        u8::try_from(alpha + u16::from(destination.alpha()) * keep / 255)
            .expect("a source-over alpha stays within a byte"),
    )
    .expect("source-over of premultiplied colors stays premultiplied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn justification_places_a_line_in_its_area() {
        assert!((Justify::Left.offset(40.0, 100) - 0.0).abs() < f32::EPSILON);
        assert!((Justify::Center.offset(40.0, 100) - 30.0).abs() < f32::EPSILON);
        assert!((Justify::Right.offset(40.0, 100) - 60.0).abs() < f32::EPSILON);
        assert!(
            (Justify::Center.offset(140.0, 100) - 0.0).abs() < f32::EPSILON,
            "text wider than its area starts at the left edge"
        );
    }

    #[test]
    fn blending_over_transparent_gives_the_premultiplied_source() {
        let transparent = PremultipliedColorU8::from_rgba(0, 0, 0, 0).expect("valid");
        let blended = blend_over(transparent, cosmic_text::Color::rgba(255, 255, 255, 128));
        assert_eq!((blended.red(), blended.alpha()), (128, 128));
    }
}
