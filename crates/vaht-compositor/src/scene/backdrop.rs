//! What the desktop shows beneath everything: the wallpaper, and the
//! pieces of it pseudo-transparent windows show through (00 §4).
//!
//! The image is decoded once, when the settings name it, and painted once
//! for each output, at its size ([`vaht_ui::Wallpaper`]). A window a rule makes
//! pseudo-transparent gets its own copy of the piece under its content,
//! drawn beneath its surface and over the windows below, so where it is
//! translucent only the wallpaper shows (fluxland's `pseudo_transparency.c`,
//! as a render pass). Each piece is its own buffer: the renderer tracks
//! damage by buffer, so one buffer drawn twice in a frame would confuse it.
//! A piece is copied again only when its window's area or the wallpaper
//! changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Logical, Rectangle, Scale};
use vaht_ui::{Color, Pixmap, Wallpaper};

use super::{PanelTexture, SceneElement};
use crate::config::WallpaperSettings;
use crate::window_manager::{Window, WindowId, WindowManager};

/// The wallpaper, and the pieces of it under pseudo-transparent windows.
#[derive(Debug)]
pub(crate) struct Backdrop {
    /// What the settings say.
    settings: WallpaperSettings,
    /// Where the configuration is, which a relative image path is from.
    config_directory: PathBuf,
    /// The decoded image; none without one, or if it could not be read.
    image: Option<Wallpaper>,
    /// The image painted for each output.
    paintings: Vec<Painting>,
    /// Counts paintings, so pieces know when theirs is stale.
    generation: u64,
    /// Each pseudo-transparent window's piece.
    pieces: HashMap<WindowId, Piece>,
}

/// The wallpaper painted for one output.
#[derive(Debug)]
struct Painting {
    /// The output's area, in global coordinates.
    area: Rectangle<i32, Logical>,
    /// The pixels.
    pixels: Pixmap,
    /// Which painting it is, counting every one.
    generation: u64,
    /// The pixels, as the renderer takes them.
    texture: PanelTexture,
}

/// The wallpaper under one pseudo-transparent window.
#[derive(Debug)]
struct Piece {
    /// Where it is, in global coordinates: the window's content, within
    /// the output.
    area: Rectangle<i32, Logical>,
    /// The painting it was copied from.
    generation: u64,
    /// Counts copies, so its texture uploads each new one.
    copies: u64,
    /// The pixels, as the renderer takes them.
    texture: PanelTexture,
}

impl Backdrop {
    /// What shows where the wallpaper does not reach, or there is none:
    /// the renderer's own clear color.
    const BACKGROUND: Color = Color::rgb(0x1a, 0x1a, 0x1a);

    /// Creates the backdrop `settings` describe, reading its image; a
    /// relative image path is from `config_directory`.
    pub(crate) fn new(settings: &WallpaperSettings, config_directory: &Path) -> Self {
        Self {
            settings: settings.clone(),
            config_directory: config_directory.to_owned(),
            image: Self::load(settings, config_directory),
            paintings: Vec::new(),
            generation: 0,
            pieces: HashMap::new(),
        }
    }

    /// Takes `settings`, reading the image again: a reload is how a
    /// changed image file is seen.
    pub(crate) fn apply(&mut self, settings: &WallpaperSettings) {
        self.settings = settings.clone();
        self.image = Self::load(settings, &self.config_directory);
        self.paintings.clear();
    }

    /// Paints the wallpaper for each output covering one of `outputs`
    /// that is not painted for already, and copies the pieces the
    /// pseudo-transparent windows `window_manager` shows need.
    pub(crate) fn prepare(
        &mut self,
        outputs: &[Rectangle<i32, Logical>],
        window_manager: &WindowManager,
    ) {
        self.paint(outputs);
        self.pieces.retain(|window, _| {
            window_manager
                .window(*window)
                .is_some_and(Window::is_pseudo_transparent)
        });
        for window in window_manager.shown_top_down() {
            if !window.is_pseudo_transparent() || !window.shows_content() {
                continue;
            }
            self.cut_piece(window);
        }
    }

    /// Paints the image for each of `outputs` not painted for, and drops
    /// the paintings of outputs gone or changed.
    fn paint(&mut self, outputs: &[Rectangle<i32, Logical>]) {
        let Some(image) = &self.image else {
            self.paintings.clear();
            return;
        };
        self.paintings
            .retain(|painting| outputs.contains(&painting.area));
        for output in outputs {
            if self
                .paintings
                .iter()
                .any(|painting| painting.area == *output)
            {
                continue;
            }
            let size = vaht_ui::Size::new(to_u32(output.size.w), to_u32(output.size.h));
            if let Some(pixels) = image.paint(size, self.settings.fit, Self::BACKGROUND) {
                self.generation += 1;
                let mut texture = PanelTexture::new();
                texture.upload_pixels(&pixels, self.generation);
                self.paintings.push(Painting {
                    area: *output,
                    pixels,
                    generation: self.generation,
                    texture,
                });
            }
        }
    }

    /// Copies the piece of wallpaper under `window`'s content, from the
    /// output showing most of it, unless its piece is up to date.
    fn cut_piece(&mut self, window: &Window) {
        let content = window.content_area();
        let under = self
            .paintings
            .iter()
            .filter_map(|painting| Some((painting, painting.area.intersection(content)?)))
            .max_by_key(|(_, area)| area.size.w * area.size.h);
        let Some((painting, area)) = under else {
            self.pieces.remove(&window.id());
            return;
        };
        let piece = self.pieces.entry(window.id()).or_insert_with(|| Piece {
            area,
            generation: 0,
            copies: 0,
            texture: PanelTexture::new(),
        });
        if piece.area == area && piece.generation == painting.generation {
            return;
        }
        let within = area.loc - painting.area.loc;
        let Some(rect) = tiny_skia::IntRect::from_xywh(
            within.x,
            within.y,
            to_u32(area.size.w),
            to_u32(area.size.h),
        ) else {
            return;
        };
        if let Some(copy) = painting.pixels.clone_rect(rect) {
            piece.area = area;
            piece.generation = painting.generation;
            piece.copies += 1;
            piece.texture.upload_pixels(&copy, piece.copies);
        }
    }

    /// Returns the elements that draw the wallpaper on each output; none
    /// without one.
    pub(crate) fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.paintings
            .iter()
            .filter_map(|painting| {
                painting
                    .texture
                    .render_element(renderer, painting.area.loc, scale, 1.0)
            })
            .collect()
    }

    /// Returns the element that draws the wallpaper under `window`, if it
    /// is pseudo-transparent and there is a wallpaper.
    pub(crate) fn piece_element<R>(
        &self,
        window: WindowId,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Option<SceneElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let piece = self.pieces.get(&window)?;
        piece
            .texture
            .render_element(renderer, piece.area.loc, scale, 1.0)
    }

    /// Reads the image `settings` name; none, with a warning, if it cannot
    /// be read.
    fn load(settings: &WallpaperSettings, config_directory: &Path) -> Option<Wallpaper> {
        let path = settings.image_path(config_directory)?;
        match Wallpaper::load(&path) {
            Ok(image) => Some(image),
            Err(error) => {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "no wallpaper: its image could not be read"
                );
                None
            }
        }
    }
}

/// Converts a non-negative extent to a size.
fn to_u32(extent: i32) -> u32 {
    u32::try_from(extent).unwrap_or(0)
}
