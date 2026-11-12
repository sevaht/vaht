//! An output's image, as a capture client receives it.

use std::path::Path;

/// A pixel rectangle: left and top columns and rows, then right and bottom,
/// all inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// The leftmost column.
    pub left: u32,
    /// The topmost row.
    pub top: u32,
    /// The rightmost column.
    pub right: u32,
    /// The bottommost row.
    pub bottom: u32,
}

/// A captured image: rows of 32-bit BGRA pixels (ARGB8888 in memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    /// Its width in pixels.
    width: u32,
    /// Its height in pixels.
    height: u32,
    /// The pixels, row by row, four bytes each.
    pixels: Vec<u8>,
}

impl Screenshot {
    /// Bytes per pixel.
    const BYTES_PER_PIXEL: usize = 4;

    /// Wraps packed ARGB8888 rows.
    ///
    /// # Panics
    ///
    /// Panics if `pixels` is not `width` × `height` pixels.
    #[must_use]
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(
            pixels.len(),
            to_index(width) * to_index(height) * Self::BYTES_PER_PIXEL,
            "a screenshot has four bytes per pixel"
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Returns the image's width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the image's height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the red, green and blue of the pixel at `x`, `y`.
    ///
    /// # Panics
    ///
    /// Panics if the pixel lies outside the image.
    #[must_use]
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        assert!(
            x < self.width && y < self.height,
            "pixel {x},{y} is outside the screenshot"
        );
        let start = (to_index(y) * to_index(self.width) + to_index(x)) * Self::BYTES_PER_PIXEL;
        let [blue, green, red] = [
            self.pixels[start],
            self.pixels[start + 1],
            self.pixels[start + 2],
        ];
        [red, green, blue]
    }

    /// Returns the bounds of every pixel of exactly `color`, if any.
    #[must_use]
    pub fn bounds_of(&self, color: [u8; 3]) -> Option<Bounds> {
        let mut found: Option<Bounds> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.rgb(x, y) == color {
                    let bounds = found.get_or_insert(Bounds {
                        left: x,
                        top: y,
                        right: x,
                        bottom: y,
                    });
                    bounds.left = bounds.left.min(x);
                    bounds.right = bounds.right.max(x);
                    bounds.bottom = y;
                }
            }
        }
        found
    }

    /// Returns whether the two screenshots differ anywhere inside `bounds`.
    ///
    /// # Panics
    ///
    /// Panics if `bounds` reaches outside either screenshot.
    #[must_use]
    pub fn differs_within(&self, other: &Self, bounds: Bounds) -> bool {
        (bounds.top..=bounds.bottom)
            .any(|y| (bounds.left..=bounds.right).any(|x| self.rgb(x, y) != other.rgb(x, y)))
    }

    /// Writes the image as a PNG, to look at when a test fails.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be written.
    pub fn save_png(&self, path: &Path) {
        let mut pixmap =
            tiny_skia::Pixmap::new(self.width, self.height).expect("a screenshot has an area");
        let (targets, _) = pixmap
            .data_mut()
            .as_chunks_mut::<{ Self::BYTES_PER_PIXEL }>();
        let (sources, _) = self.pixels.as_chunks::<{ Self::BYTES_PER_PIXEL }>();
        for (target, &[blue, green, red, _]) in targets.iter_mut().zip(sources) {
            // BGRA to RGBA; captures are opaque.
            *target = [red, green, blue, u8::MAX];
        }
        pixmap
            .save_png(path)
            .expect("the screenshot can be written");
    }
}

/// Converts a pixel coordinate to an index.
fn to_index(value: u32) -> usize {
    usize::try_from(value).expect("pixel coordinates fit in usize")
}
