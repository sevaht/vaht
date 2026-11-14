//! The pointer's picture: what clients ask for, drawn from the cursor theme.
//!
//! A client says what the pointer over it looks like: a named cursor
//! (`default`, `text`, `pointer`…), a surface of its own, or nothing.
//! [`Cursor`] keeps what was asked and makes the render elements that draw
//! it; a backend without a host to draw the pointer (DRM) adds them on
//! top, as does a screenshot that asks for the pointer. Named cursors come
//! from the xcursor theme `XCURSOR_THEME` names at `XCURSOR_SIZE` (the
//! `default` theme at 24 pixels without them), and a cursor the theme
//! lacks is drawn as vaht's own arrow, so the pointer never disappears.
//! Which picture of an animated cursor shows, and which of a theme's sizes
//! is used, are worked out from plain values ([`CursorFrames`]).

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus};
use smithay::reexports::wayland_server::Resource as _;
use smithay::utils::{Logical, Physical, Point, Scale, Transform};
use smithay::wayland::compositor::with_states;

use crate::scene::SceneElement;

/// One picture of a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorFrame {
    /// Its width and height, in pixels.
    pub(crate) size: (u32, u32),
    /// The point of it that is the pointer's position.
    pub(crate) hotspot: (u32, u32),
    /// How long it shows before the next, for an animated cursor.
    pub(crate) delay: Duration,
    /// Its pixels: premultiplied ARGB8888 as Wayland lays it out in
    /// memory (blue, green, red, alpha).
    pub(crate) pixels: Vec<u8>,
}

/// The pictures of one cursor at one size: one, or an animation's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorFrames {
    /// The pictures, in order; never empty.
    frames: Vec<CursorFrame>,
}

/// The xcursor theme in use, and the cursors read from it so far.
#[derive(Debug)]
struct CursorTheme {
    /// The theme.
    theme: xcursor::CursorTheme,
    /// The cursor size wanted, in pixels.
    size: u32,
    /// Each cursor asked for, read once; the arrow where the theme lacks
    /// it.
    loaded: HashMap<CursorIcon, CursorFrames>,
}

/// The pointer's picture.
#[derive(Debug)]
pub(crate) struct Cursor {
    /// What the client under the pointer asked for.
    status: CursorImageStatus,
    /// Where named cursors come from.
    theme: CursorTheme,
    /// Each named cursor picture, as the renderer takes it.
    buffers: HashMap<(CursorIcon, usize), MemoryRenderBuffer>,
    /// The named cursor picture showing now, and its hotspot, as
    /// [`Cursor::prepare`] chose it.
    showing: Option<(MemoryRenderBuffer, Point<f64, Logical>)>,
}

impl CursorFrames {
    /// The arrow's side, at scale 1, in pixels.
    const ARROW_SIZE: u32 = 24;

    /// The pictures of `images` (each a nominal size, and a picture) at the
    /// nominal size nearest `size`, in order; none without any.
    pub(crate) fn nearest_size(images: Vec<(u32, CursorFrame)>, size: u32) -> Option<Self> {
        let nearest = images
            .iter()
            .map(|(nominal, _)| *nominal)
            .min_by_key(|nominal| (nominal.abs_diff(size), *nominal))?;
        let frames: Vec<CursorFrame> = images
            .into_iter()
            .filter(|(nominal, _)| *nominal == nearest)
            .map(|(_, frame)| frame)
            .collect();
        Some(Self { frames })
    }

    /// vaht's own arrow, `size` pixels square: shown where the theme has
    /// no cursor.
    pub(crate) fn arrow(size: u32) -> Self {
        let edge = size.max(8);
        let pixmap = vaht_ui::Pixmap::new(edge, edge)
            .map(|mut pixmap| {
                Self::paint_arrow(&mut pixmap);
                pixmap
            })
            .expect("an arrow of at least 8 pixels can be painted");
        let pixels = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| [pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()])
            .collect();
        Self {
            frames: vec![CursorFrame {
                size: (edge, edge),
                hotspot: (1, 1),
                delay: Duration::ZERO,
                pixels,
            }],
        }
    }

    /// Paints a black arrow, outlined in white, pointing up and left from
    /// the top left corner of `pixmap`.
    fn paint_arrow(pixmap: &mut vaht_ui::Pixmap) {
        #[expect(clippy::cast_precision_loss, reason = "a cursor is a few dozen pixels")]
        let unit = pixmap.width() as f32 / Self::ARROW_SIZE as f32;
        let points = [
            (1.0, 1.0),
            (1.0, 18.0),
            (5.5, 14.0),
            (8.5, 21.0),
            (11.5, 19.8),
            (8.5, 13.0),
            (14.5, 13.0),
        ];
        let mut path = tiny_skia::PathBuilder::new();
        for (index, (x, y)) in points.iter().enumerate() {
            if index == 0 {
                path.move_to(x * unit, y * unit);
            } else {
                path.line_to(x * unit, y * unit);
            }
        }
        path.close();
        let Some(path) = path.finish() else {
            return;
        };
        let mut paint = tiny_skia::Paint {
            anti_alias: true,
            ..tiny_skia::Paint::default()
        };
        paint.set_color_rgba8(0, 0, 0, 255);
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
        paint.set_color_rgba8(255, 255, 255, 255);
        let stroke = tiny_skia::Stroke {
            width: unit.max(1.0),
            ..tiny_skia::Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &paint,
            &stroke,
            tiny_skia::Transform::identity(),
            None,
        );
    }

    /// Returns which picture shows `elapsed` after the animation began,
    /// and the picture: the animation repeats.
    pub(crate) fn frame_at(&self, elapsed: Duration) -> (usize, &CursorFrame) {
        let total: Duration = self.frames.iter().map(|frame| frame.delay).sum();
        if total.is_zero() || self.frames.len() == 1 {
            return (0, &self.frames[0]);
        }
        let mut within = Duration::from_nanos(
            u64::try_from(elapsed.as_nanos() % total.as_nanos()).unwrap_or_default(),
        );
        for (index, frame) in self.frames.iter().enumerate() {
            if within < frame.delay {
                return (index, frame);
            }
            within -= frame.delay;
        }
        (0, &self.frames[0])
    }

    /// Returns whether it changes over time.
    pub(crate) fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }
}

impl CursorTheme {
    /// The cursor size without `XCURSOR_SIZE`, in pixels.
    const DEFAULT_SIZE: u32 = 24;

    /// The theme the environment names (`XCURSOR_THEME`, `XCURSOR_SIZE`),
    /// or `default` at 24 pixels.
    fn from_environment() -> Self {
        let name = std::env::var("XCURSOR_THEME")
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "default".to_owned());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.parse().ok())
            .filter(|size| *size > 0)
            .unwrap_or(Self::DEFAULT_SIZE);
        Self {
            theme: xcursor::CursorTheme::load(&name),
            size,
            loaded: HashMap::new(),
        }
    }

    /// Returns the pictures of `icon`: the theme's, under its name or one
    /// of its other names, or the arrow.
    fn frames(&mut self, icon: CursorIcon) -> &CursorFrames {
        let (theme, size) = (&self.theme, self.size);
        self.loaded.entry(icon).or_insert_with(|| {
            std::iter::once(icon.name())
                .chain(icon.alt_names().iter().copied())
                .find_map(|name| Self::read(theme, name, size))
                .unwrap_or_else(|| {
                    tracing::debug!(cursor = icon.name(), "the cursor theme lacks it");
                    CursorFrames::arrow(size)
                })
        })
    }

    /// Reads the cursor `name` from `theme` at the size nearest `size`.
    fn read(theme: &xcursor::CursorTheme, name: &str, size: u32) -> Option<CursorFrames> {
        let path = theme.load_icon(name)?;
        let content = std::fs::read(&path)
            .inspect_err(
                |error| tracing::debug!(%error, path = %path.display(), "unreadable cursor"),
            )
            .ok()?;
        let images = xcursor::parser::parse_xcursor(&content)?;
        let frames = images
            .into_iter()
            .map(|image| {
                (
                    image.size,
                    CursorFrame {
                        size: (image.width, image.height),
                        hotspot: (image.xhot, image.yhot),
                        delay: Duration::from_millis(u64::from(image.delay)),
                        pixels: image.pixels_rgba,
                    },
                )
            })
            .collect();
        CursorFrames::nearest_size(frames, size)
    }
}

impl Cursor {
    /// The cursor of the theme the environment names, showing the default
    /// arrow until a client asks for another.
    pub(crate) fn from_environment() -> Self {
        Self {
            status: CursorImageStatus::default_named(),
            theme: CursorTheme::from_environment(),
            buffers: HashMap::new(),
            showing: None,
        }
    }

    /// Returns the default arrow's first picture, for Xwayland's root
    /// window: what X programs show where they set no cursor.
    pub(crate) fn default_frame(&mut self) -> CursorFrame {
        self.theme
            .frames(CursorIcon::Default)
            .frame_at(Duration::ZERO)
            .1
            .clone()
    }

    /// Shows what a client asked for from now on.
    pub(crate) fn set_status(&mut self, status: CursorImageStatus) {
        self.status = status;
    }

    /// Returns whether the picture changes by itself over time, so the
    /// outputs it is on must be drawn again for it.
    pub(crate) fn is_animated(&mut self) -> bool {
        match &self.status {
            CursorImageStatus::Named(icon) => self.theme.frames(*icon).is_animated(),
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => false,
        }
    }

    /// Chooses the picture to show `elapsed` into an animation: reads a
    /// named cursor from the theme the first time, and goes back to the
    /// default arrow when the client's own cursor surface is gone.
    pub(crate) fn prepare(&mut self, elapsed: Duration) {
        if let CursorImageStatus::Surface(surface) = &self.status
            && !surface.is_alive()
        {
            self.status = CursorImageStatus::default_named();
        }
        self.showing = match &self.status {
            CursorImageStatus::Named(icon) => {
                let icon = *icon;
                let (index, frame) = self.theme.frames(icon).frame_at(elapsed);
                let hotspot = Point::from((f64::from(frame.hotspot.0), f64::from(frame.hotspot.1)));
                let size = (
                    i32::try_from(frame.size.0).unwrap_or(1),
                    i32::try_from(frame.size.1).unwrap_or(1),
                );
                let pixels = &frame.pixels;
                let buffer = self
                    .buffers
                    .entry((icon, index))
                    .or_insert_with(|| {
                        MemoryRenderBuffer::from_slice(
                            pixels,
                            Fourcc::Argb8888,
                            size,
                            1,
                            Transform::Normal,
                            None,
                        )
                    })
                    .clone();
                Some((buffer, hotspot))
            }
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => None,
        };
    }

    /// Returns the elements that draw the pointer at `location` (global
    /// coordinates) at `scale`, as last prepared.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportAll + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        match &self.status {
            CursorImageStatus::Hidden => Vec::new(),
            CursorImageStatus::Named(_) => {
                let Some((buffer, hotspot)) = &self.showing else {
                    return Vec::new();
                };
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    (location - *hotspot).to_physical(scale),
                    buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                )
                .inspect_err(|error| tracing::warn!(?error, "could not draw the pointer"))
                .ok()
                .map(SceneElement::Panel)
                .into_iter()
                .collect()
            }
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<std::sync::Mutex<CursorImageAttributes>>()
                        .and_then(|attributes| {
                            attributes.lock().ok().map(|attributes| attributes.hotspot)
                        })
                })
                .unwrap_or_default();
                let corner: Point<i32, Physical> = (location - hotspot.to_f64())
                    .to_physical(scale)
                    .to_i32_round();
                render_elements_from_surface_tree(
                    renderer,
                    surface,
                    corner,
                    scale,
                    1.0,
                    Kind::Cursor,
                )
                .into_iter()
                .map(SceneElement::Surface)
                .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture `side` pixels square, shown for `delay_ms`.
    fn frame(side: u32, delay_ms: u64) -> CursorFrame {
        CursorFrame {
            size: (side, side),
            hotspot: (0, 0),
            delay: Duration::from_millis(delay_ms),
            pixels: vec![0; usize::try_from(side * side * 4).expect("small")],
        }
    }

    #[test]
    fn the_size_nearest_the_one_wanted_is_used() {
        let images = vec![(24, frame(24, 0)), (32, frame(32, 0)), (48, frame(48, 0))];
        let frames = CursorFrames::nearest_size(images, 30).expect("some");
        assert_eq!(frames.frame_at(Duration::ZERO).1.size, (32, 32));
        assert!(CursorFrames::nearest_size(Vec::new(), 24).is_none());
    }

    #[test]
    fn an_animation_steps_through_its_pictures_and_repeats() {
        let images = vec![(24, frame(24, 100)), (24, frame(24, 50))];
        let frames = CursorFrames::nearest_size(images, 24).expect("some");
        assert!(frames.is_animated());
        let index = |millis| frames.frame_at(Duration::from_millis(millis)).0;
        assert_eq!(
            [index(0), index(99), index(100), index(149), index(150)],
            [0, 0, 1, 1, 0]
        );
    }

    #[test]
    fn the_arrow_has_its_tip_at_the_hotspot() {
        let arrow = CursorFrames::arrow(24);
        let (_, frame) = arrow.frame_at(Duration::ZERO);
        assert_eq!(frame.size, (24, 24));
        let alpha_at =
            |x: u32, y: u32| frame.pixels[usize::try_from((y * 24 + x) * 4 + 3).expect("small")];
        assert!(alpha_at(2, 4) > 0, "the arrow is drawn below its tip");
        assert_eq!(alpha_at(22, 2), 0, "and nowhere near the far corner");
    }
}
