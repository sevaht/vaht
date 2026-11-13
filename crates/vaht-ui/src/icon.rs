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

mod store;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

pub use self::store::IconStore;

/// A square picture, shared by every copy.
#[derive(Clone)]
pub struct Icon {
    /// The pixels, `size` × `size`.
    pixels: Arc<Pixmap>,
}

impl Icon {
    /// Returns `pixmap`, fitted into a square `size` wide and centered:
    /// scaled down or up to fit, keeping its shape.
    #[must_use]
    pub fn fitted(pixmap: &Pixmap, size: u32) -> Option<Self> {
        let mut square = Pixmap::new(size, size)?;
        let (width, height) = (pixels(pixmap.width()), pixels(pixmap.height()));
        let extent = pixels(size);
        let scale = extent / width.max(height);
        let transform = Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            (extent - width * scale) / 2.0,
            (extent - height * scale) / 2.0,
        );
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        square.draw_pixmap(0, 0, pixmap.as_ref(), &paint, transform, None);
        Some(Self {
            pixels: Arc::new(square),
        })
    }

    /// Reads the PNG or SVG file at `path` as an icon `size` wide.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or decoded, or is
    /// neither a PNG nor an SVG.
    pub fn load(path: &Path, size: u32) -> Result<Self, IconError> {
        let data = std::fs::read(path).map_err(|error| IconError::Read {
            path: path.to_path_buf(),
            error,
        })?;
        let decoded = match path.extension().and_then(|extension| extension.to_str()) {
            Some("png") => Pixmap::decode_png(&data).map_err(|error| IconError::Decode {
                path: path.to_path_buf(),
                error: Box::new(error),
            })?,
            Some("svg") => Self::render_svg(path, &data, size)?,
            _ => return Err(IconError::Format(path.to_path_buf())),
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
    fn copies_are_the_same_icon_and_decodes_are_not() {
        let icon = Icon::fitted(&red(4, 4), 4).expect("an icon");
        assert_eq!(icon.clone(), icon);
        assert_ne!(Icon::fitted(&red(4, 4), 4).expect("an icon"), icon);
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
