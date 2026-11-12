//! Shaping, measuring and drawing text, with Fluxbox's halo and shadow.
//!
//! A [`Typesetter`] owns cosmic-text's font database and glyph cache. Loading
//! the system's fonts takes tens of milliseconds, so there is one typesetter,
//! lent to whatever is being drawn. Text is drawn into a layer the size of its
//! area, which clips it there, and the layer is then composited.

use std::fmt;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style as SlantStyle, SwashCache, Weight,
    Wrap,
};
use tiny_skia::{Pixmap, PixmapPaint, PremultipliedColorU8, Transform};

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

impl TextEffect {
    /// Returns how far the effect reaches beyond the letters, in pixels.
    #[must_use]
    pub fn reach(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Halo(_) => 1,
            Self::Shadow {
                offset_x, offset_y, ..
            } => offset_x.unsigned_abs().max(offset_y.unsigned_abs()),
        }
    }
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

    /// Where a halo's copies of the letters go: one pixel away in every
    /// direction, the 1-pixel outline Fluxbox and fluxland draw.
    const HALO_DIRECTIONS: [(f32, f32); 8] = [
        (-1.0, -1.0),
        (0.0, -1.0),
        (1.0, -1.0),
        (-1.0, 0.0),
        (1.0, 0.0),
        (-1.0, 1.0),
        (0.0, 1.0),
        (1.0, 1.0),
    ];

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
        // The layer extends past the area by the effect's reach, so a halo
        // or shadow around the outermost letters is never clipped.
        let margin = style.effect.reach();
        let extra = 2 * u32::from(margin);
        let Some(mut layer) = Pixmap::new(area.width + extra, area.height + extra) else {
            return;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "panel heights are far below 2^24"
        )]
        let line_height = area.height as f32;
        let margin_pixels = f32::from(margin);
        let mut buffer = self.lay_out(text, &style.font, line_height);
        let Some(line_width) = buffer.layout_runs().next().map(|line| line.line_w) else {
            return;
        };
        let origin = (
            style.justify.offset(line_width, area.width) + margin_pixels,
            margin_pixels,
        );
        let at = |shift_x: f32, shift_y: f32| (origin.0 + shift_x, origin.1 + shift_y);
        match style.effect {
            TextEffect::None => {}
            // The letters' own coverage, drawn one pixel away in each of the
            // eight directions: an outline exactly aligned with the letters.
            TextEffect::Halo(color) => {
                for (shift_x, shift_y) in Self::HALO_DIRECTIONS {
                    self.fill_glyphs(&mut layer, &mut buffer, at(shift_x, shift_y), color);
                }
            }
            TextEffect::Shadow {
                color,
                offset_x,
                offset_y,
            } => self.fill_glyphs(
                &mut layer,
                &mut buffer,
                at(f32::from(offset_x), f32::from(offset_y)),
                color,
            ),
        }
        self.fill_glyphs(&mut layer, &mut buffer, origin, style.color);
        let margin_offset = i32::from(margin);
        target.draw_pixmap(
            area.x - margin_offset,
            area.y - margin_offset,
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

    /// A pixel rectangle's left, top, right and bottom columns and rows.
    type Bounds = (u32, u32, u32, u32);

    /// The bounds of the pixels `matches` selects.
    fn bounds(pixmap: &Pixmap, matches: impl Fn(PremultipliedColorU8) -> bool) -> Bounds {
        let mut found = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..pixmap.height() {
            for x in 0..pixmap.width() {
                if pixmap.pixel(x, y).is_some_and(&matches) {
                    found = (
                        found.0.min(x),
                        found.1.min(y),
                        found.2.max(x),
                        found.3.max(y),
                    );
                }
            }
        }
        found
    }

    /// The test text's style: white bold letters with a black halo.
    fn haloed_style() -> TextStyle {
        TextStyle {
            font: Font {
                family: "DejaVu Sans".to_owned(),
                size_points: 12,
                weight: FontWeight::Bold,
                is_italic: false,
            },
            color: Color::WHITE,
            effect: TextEffect::Halo(Color::BLACK),
            justify: Justify::Left,
        }
    }

    /// Draws "I" in `area` of a grey canvas; returns the bounds of the
    /// letter (nearly white pixels) and of its halo (nearly black ones).
    fn draw_haloed_letter(typesetter: &mut Typesetter, area: Rect) -> (Bounds, Bounds) {
        let mut target = Pixmap::new(60, 30).expect("a small pixmap");
        target.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 255));
        typesetter.draw(&mut target, "I", &haloed_style(), area);
        (
            bounds(&target, |pixel| pixel.red() >= 200),
            bounds(&target, |pixel| pixel.red() < 40),
        )
    }

    #[test]
    fn a_halo_surrounds_the_letters_evenly() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let (letter, halo) = draw_haloed_letter(&mut typesetter, Rect::new(20, 5, 30, 20));
        let (left, top) = (letter.0 - halo.0, letter.1 - halo.1);
        let (right, bottom) = (halo.2 - letter.2, halo.3 - letter.3);
        assert!(left >= 1 && top >= 1, "the halo lies outside the letter");
        assert_eq!(left, right, "as much halo left as right");
        assert_eq!(top, bottom, "as much halo above as below");
    }

    #[test]
    fn an_effect_reaching_past_its_area_is_not_clipped() {
        let mut typesetter = Typesetter::new(Typesetter::DEFAULT_DOTS_PER_INCH);
        let mut target = Pixmap::new(60, 30).expect("a small pixmap");
        target.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 255));
        // A shadow four pixels left of left-justified text lies partly left
        // of the area, which is where clipping would cut it off.
        let style = TextStyle {
            effect: TextEffect::Shadow {
                color: Color::BLACK,
                offset_x: -4,
                offset_y: 0,
            },
            ..haloed_style()
        };
        let area = Rect::new(20, 5, 30, 20);
        typesetter.draw(&mut target, "I", &style, area);
        let shadow = bounds(&target, |pixel| pixel.red() < 40);
        assert!(
            i32::try_from(shadow.0).expect("small") < area.x,
            "the shadow extends left of the area instead of being cut off"
        );
    }

    #[test]
    fn blending_over_transparent_gives_the_premultiplied_source() {
        let transparent = PremultipliedColorU8::from_rgba(0, 0, 0, 0).expect("valid");
        let blended = blend_over(transparent, cosmic_text::Color::rgba(255, 255, 255, 128));
        assert_eq!((blended.red(), blended.alpha()), (128, 128));
    }
}
