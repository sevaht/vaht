//! A wallpaper: an image, decoded once, painted to fill an output.
//!
//! [`Wallpaper`] holds the decoded image; [`Wallpaper::paint`] scales it
//! into a pixmap the size of an output as its [`WallpaperFit`] says, over a
//! background color where the image does not reach. The compositor paints
//! once per output size and draws the result as the lowest thing on the
//! output.

use std::path::{Path, PathBuf};

use tiny_skia::{
    FilterQuality, Pattern, Pixmap, PixmapPaint, Rect as SkiaRect, SpreadMode, Transform,
};

use crate::color::Color;
use crate::geometry::Size;

/// How a wallpaper fills an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Deserialize, strum::EnumIter)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperFit {
    /// Scaled to cover the output, keeping its shape; what overflows is
    /// cut off, evenly on both sides.
    #[default]
    Fill,
    /// Scaled to fit inside the output, keeping its shape; the background
    /// shows beside it.
    Fit,
    /// At its own size, in the middle.
    Center,
    /// At its own size, repeated from the top-left corner.
    Tile,
    /// Scaled to the output's shape, whatever its own.
    Stretch,
}

/// A decoded wallpaper image.
#[derive(Debug, Clone, PartialEq)]
pub struct Wallpaper {
    /// The image.
    image: Pixmap,
}

impl Wallpaper {
    /// Decodes the PNG image at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a PNG image.
    pub fn load(path: &Path) -> Result<Self, WallpaperError> {
        let data = std::fs::read(path).map_err(|source| WallpaperError::Read {
            path: path.to_owned(),
            source,
        })?;
        let image = Pixmap::decode_png(&data).map_err(|error| WallpaperError::Decode {
            path: path.to_owned(),
            reason: error.to_string(),
        })?;
        Ok(Self { image })
    }

    /// Wraps an image already decoded.
    #[must_use]
    pub const fn from_pixmap(image: Pixmap) -> Self {
        Self { image }
    }

    /// Returns the image painted into a pixmap of `size` as `fit` says,
    /// over `background`; none for an empty size.
    #[must_use]
    pub fn paint(&self, size: Size, fit: WallpaperFit, background: Color) -> Option<Pixmap> {
        let mut target = Pixmap::new(size.width, size.height)?;
        target.fill(background.to_skia());
        #[expect(
            clippy::cast_precision_loss,
            reason = "output and image sizes are far below 2^24, where f32 stays exact"
        )]
        let (output_width, output_height, image_width, image_height) = (
            size.width as f32,
            size.height as f32,
            self.image.width() as f32,
            self.image.height() as f32,
        );
        let (scale_x, scale_y) = match fit {
            WallpaperFit::Fill => {
                let scale = (output_width / image_width).max(output_height / image_height);
                (scale, scale)
            }
            WallpaperFit::Fit => {
                let scale = (output_width / image_width).min(output_height / image_height);
                (scale, scale)
            }
            WallpaperFit::Center | WallpaperFit::Tile => (1.0, 1.0),
            WallpaperFit::Stretch => (output_width / image_width, output_height / image_height),
        };
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        if fit == WallpaperFit::Tile {
            let pattern = Pattern::new(
                self.image.as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Nearest,
                1.0,
                Transform::identity(),
            );
            let area = SkiaRect::from_xywh(0.0, 0.0, output_width, output_height)?;
            let tiles = tiny_skia::Paint {
                shader: pattern,
                ..tiny_skia::Paint::default()
            };
            target.fill_rect(area, &tiles, Transform::identity(), None);
            return Some(target);
        }
        // Centered: what does not fit is cut off evenly on both sides.
        let offset_x = (output_width - image_width * scale_x) / 2.0;
        let offset_y = (output_height - image_height * scale_y) / 2.0;
        let transform = Transform::from_row(scale_x, 0.0, 0.0, scale_y, offset_x, offset_y);
        target.draw_pixmap(0, 0, self.image.as_ref(), &paint, transform, None);
        Some(target)
    }
}

/// Why a wallpaper could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum WallpaperError {
    /// The file could not be read.
    #[error("could not read the wallpaper {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The file is not a PNG image vaht can decode.
    #[error("the wallpaper {} is not a PNG image vaht can read: {reason}", path.display())]
    Decode {
        /// The file.
        path: PathBuf,
        /// Why, as the decoder says.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 by 2 image: its left half red, its right half blue.
    fn halves() -> Wallpaper {
        let mut image = Pixmap::new(4, 2).expect("a size");
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(Color::rgb(255, 0, 0).to_skia());
        let left = SkiaRect::from_xywh(0.0, 0.0, 2.0, 2.0).expect("a rectangle");
        image.fill_rect(left, &paint, Transform::identity(), None);
        paint.set_color(Color::rgb(0, 0, 255).to_skia());
        let right = SkiaRect::from_xywh(2.0, 0.0, 2.0, 2.0).expect("a rectangle");
        image.fill_rect(right, &paint, Transform::identity(), None);
        Wallpaper::from_pixmap(image)
    }

    /// Returns the color at `x`, `y` of `pixmap`.
    fn at(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
        let pixel = pixmap.pixel(x, y).expect("inside");
        (pixel.red(), pixel.green(), pixel.blue())
    }

    #[test]
    fn fill_covers_the_output_cutting_the_overflow_evenly() {
        // A 4:2 image on a square: scaled to the height, its sides cut.
        let painted = halves()
            .paint(Size::new(40, 40), WallpaperFit::Fill, Color::BLACK)
            .expect("painted");
        assert_eq!(at(&painted, 2, 20), (255, 0, 0));
        assert_eq!(at(&painted, 37, 20), (0, 0, 255));
        assert_eq!(at(&painted, 2, 1), (255, 0, 0), "no background shows");
    }

    #[test]
    fn fit_leaves_the_background_beside_the_image() {
        let painted = halves()
            .paint(Size::new(40, 40), WallpaperFit::Fit, Color::BLACK)
            .expect("painted");
        assert_eq!(at(&painted, 20, 2), (0, 0, 0), "above the image");
        assert_eq!(at(&painted, 5, 20), (255, 0, 0));
    }

    #[test]
    fn center_and_tile_keep_the_image_size() {
        let wallpaper = halves();
        let centered = wallpaper
            .paint(Size::new(8, 4), WallpaperFit::Center, Color::BLACK)
            .expect("painted");
        assert_eq!(at(&centered, 0, 0), (0, 0, 0));
        assert_eq!(at(&centered, 2, 1), (255, 0, 0));
        assert_eq!(at(&centered, 5, 2), (0, 0, 255));
        let tiled = wallpaper
            .paint(Size::new(8, 4), WallpaperFit::Tile, Color::BLACK)
            .expect("painted");
        assert_eq!(at(&tiled, 4, 3), (255, 0, 0), "the second tile");
        assert_eq!(at(&tiled, 7, 3), (0, 0, 255));
    }

    #[test]
    fn stretch_takes_the_outputs_shape() {
        let painted = halves()
            .paint(Size::new(10, 40), WallpaperFit::Stretch, Color::BLACK)
            .expect("painted");
        assert_eq!(at(&painted, 1, 39), (255, 0, 0));
        assert_eq!(at(&painted, 9, 0), (0, 0, 255));
    }
}
