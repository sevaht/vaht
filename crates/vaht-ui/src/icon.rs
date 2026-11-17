//! Application icons: pictures decoded once, and finding them by name.
//!
//! An [`Icon`] is a square picture, decoded from a PNG or rendered from an
//! SVG at the size it is drawn, and shared: copies point at the same
//! pixels, and two icons are equal when they are the same picture. An
//! [`IconStore`] finds an application's icon the way desktops do: the
//! desktop entry for its app id names the icon, and the name is looked up
//! in the `hicolor` theme and the `pixmaps` directories of the XDG data
//! directories (`store.rs`). A widget shows one beside its text
//! ([`WidgetIcon`]).

mod resample;
mod store;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tiny_skia::{Pixmap, PixmapPaint, Transform};

use self::resample::resample;

pub use self::store::IconStore;

/// A picture at each size it was drawn at, each made once.
type SizedPictures = Arc<Mutex<Vec<(u32, Arc<Pixmap>)>>>;

/// A square picture, shared by every copy.
#[derive(Clone)]
pub struct Icon {
    /// The pixels, `size` × `size`.
    pixels: Arc<Pixmap>,
    /// The picture resized to the sizes it was drawn at, each made once:
    /// a big picture a program sent is drawn small again and again.
    sized: SizedPictures,
}

impl Icon {
    /// Returns `pixmap`, fitted into a square `size` wide and centered:
    /// scaled down or up to fit, keeping its shape.
    #[must_use]
    pub fn fitted(pixmap: &Pixmap, size: u32) -> Option<Self> {
        let (width, height) = (pixmap.width(), pixmap.height());
        let longer = width.max(height);
        if longer == 0 || size == 0 {
            return None;
        }
        // The longer side becomes `size`; the shorter keeps the shape.
        let scaled = |side: u32| {
            let rounded =
                (u64::from(side) * u64::from(size) + u64::from(longer) / 2) / u64::from(longer);
            u32::try_from(rounded).unwrap_or(size).clamp(1, size)
        };
        let (fitted_width, fitted_height) = (scaled(width), scaled(height));
        let resized = resample(pixmap, fitted_width, fitted_height)?;
        let square = if (fitted_width, fitted_height) == (size, size) {
            resized
        } else {
            let mut square = Pixmap::new(size, size)?;
            let offset = |fitted: u32| i32::try_from((size - fitted) / 2).unwrap_or(0);
            square.draw_pixmap(
                offset(fitted_width),
                offset(fitted_height),
                resized.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            square
        };
        Some(Self {
            pixels: Arc::new(square),
            sized: Arc::default(),
        })
    }

    /// Returns the picture `side` pixels square: its own pixels at its own
    /// size, else resized with a Lanczos filter widened by the scale, once
    /// per size; none for 0.
    #[must_use]
    pub fn pixels_at(&self, side: u32) -> Option<Arc<Pixmap>> {
        /// How many sizes are kept: icons are drawn at a few at most.
        const KEPT_SIZES: usize = 4;
        if side == self.size() {
            return Some(Arc::clone(&self.pixels));
        }
        let mut sized = self.sized.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, pixels)) = sized.iter().find(|(kept, _)| *kept == side) {
            return Some(Arc::clone(pixels));
        }
        let pixels = Arc::new(resample(&self.pixels, side, side)?);
        if sized.len() == KEPT_SIZES {
            sized.remove(0);
        }
        sized.push((side, Arc::clone(&pixels)));
        Some(pixels)
    }

    /// Returns the picture `width` by `height` whose pixels `bytes` holds,
    /// four bytes each in network byte order (alpha, red, green, blue), as
    /// tray items send theirs; none if the bytes are not exactly that.
    #[must_use]
    pub fn from_argb_bytes(width: u32, height: u32, bytes: &[u8]) -> Option<Self> {
        let mut pixmap = Pixmap::new(width, height)?;
        let (pixels, rest) = bytes.as_chunks::<4>();
        if pixels.len() != pixmap.pixels().len() || !rest.is_empty() {
            return None;
        }
        for (target, [alpha, red, green, blue]) in pixmap.pixels_mut().iter_mut().zip(pixels) {
            *target = tiny_skia::ColorU8::from_rgba(*red, *green, *blue, *alpha).premultiply();
        }
        Self::fitted(&pixmap, width.max(height))
    }

    /// Returns the picture `width` by `height` whose pixels `bytes` holds
    /// as Wayland's ARGB8888 lays them out in memory: blue, green, red and
    /// alpha, premultiplied, rows packed; none if the bytes are not exactly
    /// that.
    #[must_use]
    pub fn from_argb8888(width: u32, height: u32, bytes: &[u8]) -> Option<Self> {
        let mut pixmap = Pixmap::new(width, height)?;
        let (pixels, rest) = bytes.as_chunks::<4>();
        if pixels.len() != pixmap.pixels().len() || !rest.is_empty() {
            return None;
        }
        for (target, [blue, green, red, alpha]) in pixmap.pixels_mut().iter_mut().zip(pixels) {
            // A client's premultiplied color may exceed its alpha; clamped,
            // it is always a premultiplied color.
            let channel = |value: u8| value.min(*alpha);
            *target = tiny_skia::PremultipliedColorU8::from_rgba(
                channel(*red),
                channel(*green),
                channel(*blue),
                *alpha,
            )
            .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
        }
        Self::fitted(&pixmap, width.max(height))
    }

    /// Reads the SVG or raster image (any format [`crate::raster`] reads)
    /// at `path` as an icon `size` wide.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or decoded.
    pub fn load(path: &Path, size: u32) -> Result<Self, IconError> {
        let data = std::fs::read(path).map_err(|error| IconError::Read {
            path: path.to_path_buf(),
            error,
        })?;
        let decoded = match path.extension().and_then(|extension| extension.to_str()) {
            Some("svg") => Self::render_svg(path, &data, size)?,
            _ => crate::raster::decode(&data, path).map_err(|error| IconError::Decode {
                path: path.to_path_buf(),
                error: Box::new(error),
            })?,
        };
        Self::fitted(&decoded, size).ok_or(IconError::Empty(path.to_path_buf()))
    }

    /// Returns how wide (and tall) the icon is.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.pixels.width()
    }

    /// Returns the pixels.
    #[must_use]
    pub fn pixels(&self) -> &Pixmap {
        &self.pixels
    }

    /// Renders the SVG `data`, read from `path`, at `size` along its longer
    /// side.
    fn render_svg(path: &Path, data: &[u8], size: u32) -> Result<Pixmap, IconError> {
        let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).map_err(
            |error| IconError::Decode {
                path: path.to_path_buf(),
                error: Box::new(error),
            },
        )?;
        let natural = tree.size();
        let scale = pixels(size) / natural.width().max(natural.height());
        let mut pixmap = Pixmap::new(size, size).ok_or(IconError::Empty(path.to_path_buf()))?;
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }
}

impl PartialEq for Icon {
    /// Icons are equal when they are the same picture: decoding a file
    /// twice gives two icons.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pixels, &other.pixels)
    }
}

impl Eq for Icon {}

impl fmt::Debug for Icon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Icon")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

/// An icon shown beside a widget's text, and how strongly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetIcon {
    /// The picture.
    pub icon: Icon,
    /// How it is drawn.
    pub emphasis: IconEmphasis,
}

impl WidgetIcon {
    /// How far in from a widget's top, bottom and left edges its icon is
    /// drawn, and the gap after it: Fluxbox's iconbar draws its icon one
    /// pixel in.
    const MARGIN: u32 = 1;

    /// Returns where the icon of a widget covering `rect` goes: a square at
    /// its left, a pixel in from the edges.
    #[must_use]
    pub fn square_in(rect: crate::Rect) -> crate::Rect {
        let side = Self::side(rect.height);
        crate::Rect::new(
            rect.x.saturating_add_unsigned(Self::MARGIN),
            rect.y.saturating_add_unsigned(Self::MARGIN),
            side,
            side,
        )
    }

    /// Returns how wide (and tall) the icon of a widget `height` tall is.
    #[must_use]
    pub const fn side(height: u32) -> u32 {
        height.saturating_sub(2 * Self::MARGIN)
    }

    /// Returns how much of a widget `height` tall its icon takes from the
    /// left, the margins on both sides of it included.
    #[must_use]
    pub const fn span(height: u32) -> u32 {
        height
    }
}

/// How strongly an icon is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconEmphasis {
    /// As it is.
    Full,
    /// Faded, as a minimized window's is.
    Faded,
}

impl IconEmphasis {
    /// How opaque a faded icon is: half, as Fluxbox draws an iconified
    /// window's icon.
    const FADED_OPACITY: f32 = 0.5;

    /// Returns how opaque the icon is drawn.
    #[must_use]
    pub const fn opacity(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::Faded => Self::FADED_OPACITY,
        }
    }
}

/// Why an icon could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum IconError {
    /// The file could not be read.
    #[error("could not read the icon {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: std::io::Error,
    },
    /// The file is not a picture its format can decode.
    #[error("could not decode the icon {}", path.display())]
    Decode {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The file is neither a PNG nor an SVG.
    #[error("{} is neither a PNG nor an SVG", .0.display())]
    Format(PathBuf),
    /// The picture has no area.
    #[error("the icon {} has no area", .0.display())]
    Empty(PathBuf),
}

/// Converts a pixel count to drawing coordinates.
fn pixels(count: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "icon sizes are far below 2^24, where f32 stays exact"
    )]
    let exact = count as f32;
    exact
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    /// A red picture `width` by `height`.
    fn red(width: u32, height: u32) -> Pixmap {
        let mut pixmap = Pixmap::new(width, height).expect("an area");
        pixmap.fill(Color::rgb(255, 0, 0).to_skia());
        pixmap
    }

    #[test]
    fn a_picture_is_fitted_into_its_square_keeping_its_shape() {
        let icon = Icon::fitted(&red(40, 20), 20).expect("an icon");
        assert_eq!(icon.size(), 20);
        let alpha = |x, y| icon.pixels().pixel(x, y).expect("in the icon").alpha();
        assert_eq!(alpha(10, 2), 0, "above the picture, which is half as tall");
        assert_eq!(alpha(10, 10), 255, "the picture, centered");
    }

    #[test]
    fn network_order_pixels_become_an_icon() {
        let half_red = [128_u8, 255, 0, 0];
        let icon = Icon::from_argb_bytes(2, 2, &half_red.repeat(4)).expect("whole");
        let pixel = icon.pixels().pixel(0, 0).expect("inside");
        assert_eq!((pixel.alpha(), pixel.red()), (128, 128), "premultiplied");
        assert!(
            Icon::from_argb_bytes(2, 2, &half_red).is_none(),
            "too few bytes"
        );
        assert!(Icon::from_argb_bytes(0, 2, &[]).is_none(), "no area");
    }

    #[test]
    fn wayland_pixels_become_an_icon() {
        let half_blue = [128_u8, 0, 0, 128];
        let icon = Icon::from_argb8888(2, 2, &half_blue.repeat(4)).expect("whole");
        let pixel = icon.pixels().pixel(1, 1).expect("inside");
        assert_eq!((pixel.blue(), pixel.alpha()), (128, 128));
        assert!(
            Icon::from_argb8888(2, 2, &half_blue).is_none(),
            "too few bytes"
        );
    }

    #[test]
    fn copies_are_the_same_icon_and_decodes_are_not() {
        let icon = Icon::fitted(&red(4, 4), 4).expect("an icon");
        assert_eq!(icon.clone(), icon);
        assert_ne!(Icon::fitted(&red(4, 4), 4).expect("an icon"), icon);
    }

    #[test]
    fn an_icon_is_resized_once_for_each_size_it_is_drawn_at() {
        let icon = Icon::fitted(&red(64, 64), 64).expect("an icon");
        let small = icon.pixels_at(18).expect("resized");
        assert_eq!(small.width(), 18);
        assert!(
            Arc::ptr_eq(&small, &icon.pixels_at(18).expect("kept")),
            "made once"
        );
        assert!(
            Arc::ptr_eq(&icon.pixels_at(64).expect("its own"), &icon.pixels),
            "its own size is its own pixels"
        );
    }

    #[test]
    fn png_and_svg_files_load_at_the_size_asked() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let png = directory.path().join("app.png");
        red(64, 64).save_png(&png).expect("written");
        let svg = directory.path().join("app.svg");
        std::fs::write(
            &svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">\
             <rect width=\"16\" height=\"16\" fill=\"#00f\"/></svg>",
        )
        .expect("written");
        for path in [png, svg] {
            let icon = Icon::load(&path, 18).expect("loads");
            assert_eq!(icon.size(), 18);
            assert_eq!(
                icon.pixels().pixel(9, 9).expect("inside").alpha(),
                255,
                "{}",
                path.display()
            );
        }
        assert!(matches!(
            Icon::load(&directory.path().join("app.xpm"), 18),
            Err(IconError::Read { .. })
        ));
    }
}
