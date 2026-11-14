//! An output's image, as a capture client receives it.

use std::path::Path;

use smithay_client_toolkit::reexports::client::protocol::wl_output;

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

    /// Returns the image turned back from `transform`, the way a captured
    /// output's image is turned: as the output's own orientation shows it.
    #[must_use]
    pub fn with_transform_undone(self, transform: wl_output::Transform) -> Self {
        use wl_output::Transform;
        let (flips, quarter_turns) = match transform {
            Transform::_90 => (false, 1),
            Transform::_180 => (false, 2),
            Transform::_270 => (false, 3),
            Transform::Flipped => (true, 0),
            Transform::Flipped90 => (true, 1),
            Transform::Flipped180 => (true, 2),
            Transform::Flipped270 => (true, 3),
            Transform::Normal | _ => (false, 0),
        };
        let mut image = self;
        for _ in 0..quarter_turns {
            image = image.turned_clockwise();
        }
        if flips { image.flipped() } else { image }
    }

    /// Returns the image turned a quarter clockwise.
    fn turned_clockwise(&self) -> Self {
        let (width, height) = (self.height, self.width);
        let mut pixels = vec![0; self.pixels.len()];
        for y in 0..height {
            for x in 0..width {
                // The new (x, y) shows the old (y, old height - 1 - x).
                let from = (to_index(self.height - 1 - x) * to_index(self.width) + to_index(y))
                    * Self::BYTES_PER_PIXEL;
                let to = (to_index(y) * to_index(width) + to_index(x)) * Self::BYTES_PER_PIXEL;
                pixels[to..to + Self::BYTES_PER_PIXEL]
                    .copy_from_slice(&self.pixels[from..from + Self::BYTES_PER_PIXEL]);
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Returns the image mirrored left to right.
    fn flipped(&self) -> Self {
        let row = to_index(self.width) * Self::BYTES_PER_PIXEL;
        let pixels = self
            .pixels
            .chunks(row)
            .flat_map(|line| line.chunks(Self::BYTES_PER_PIXEL).rev().flatten().copied())
            .collect();
        Self {
            width: self.width,
            height: self.height,
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
