//! Blurring what is behind the part of a surface its program asks for
//! (`ext-background-effect-v1`, `[blur]`): frosted glass.
//!
//! A [`BlurElement`] sits just beneath the surface. Smithay calls it a
//! *framebuffer effect* (`Element::is_framebuffer_effect`): when what is
//! behind it changed, the damage tracker draws everything beneath it
//! first and lets it copy those pixels (`capture_framebuffer`), and only
//! then draws it. The copy is blurred once, with a dual Kawase blur (the
//! copy halved `passes` times, then grown back, each step averaging
//! samples `spread` pixels apart), and kept in the element's cache, so a
//! surface redrawing over an unchanged background costs one texture draw.
//!
//! The work is the renderer's ([`BlurRenderer`]): OpenGL ES blurs with two
//! shaders of vaht's, on the GPU drawing the frame; the software renderer
//! (headless, the tests') cannot read back what it drew, so it blurs
//! nothing and the background shows through plain.
//!
//! Smithay has the protocol and the framebuffer-effect hooks (#1910) but no
//! blur element of its own (Smithay#1363); `tools/smithay-watch` says when
//! it gains one.

use std::cell::RefCell;
use std::hash::{Hash as _, Hasher as _};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::{CommitCounter, RendererSurfaceStateUserData};
use smithay::backend::renderer::{
    Bind as _, BlitFrame as _, Frame as _, FrameContext as _, Offscreen as _, Renderer,
    Texture as _, TextureFilter,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::background_effect::BackgroundEffectSurfaceCachedState;
use smithay::wayland::compositor::{RectangleKind, RegionAttributes, with_states};

use crate::backend::DrmRenderer;
use crate::config::BlurSettings;

/// The shader halving the background: each pixel the average of the
/// source's four around it and, four times over, its own.
const DOWNSAMPLE_SHADER: &str = include_str!("blur_down.frag");

/// The shader growing it back: each pixel a weighted average of eight
/// samples around it.
const UPSAMPLE_SHADER: &str = include_str!("blur_up.frag");

/// A renderer that can blur what it drew beneath an element.
pub(crate) trait BlurRenderer: Renderer {
    /// Copies the pixels of `area` (where the element is on the frame) and
    /// blurs them as `blur` says, keeping the result in `cache`.
    ///
    /// # Errors
    ///
    /// Returns the renderer's error if copying or blurring failed.
    fn capture_blur(
        frame: &mut Self::Frame<'_, '_>,
        area: Rectangle<i32, Physical>,
        blur: BlurSettings,
        cache: &UserDataMap,
    ) -> Result<(), Self::Error>;

    /// Draws the blurred copy kept in `cache` over `area`, within `damage`
    /// (relative to `area`); nothing if there is none yet.
    ///
    /// # Errors
    ///
    /// Returns the renderer's error if drawing failed.
    fn draw_blur(
        frame: &mut Self::Frame<'_, '_>,
        area: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        cache: &UserDataMap,
    ) -> Result<(), Self::Error>;
}

/// The blur beneath one surface.
#[derive(Debug)]
pub(crate) struct BlurElement {
    /// What the renderer knows it by: the surface's, with the shape and
    /// strength, so a new shape or strength blurs anew.
    id: Id,
    /// Where it is: around every part of the surface that asks for blur.
    geometry: Rectangle<i32, Physical>,
    /// The parts asking for blur, relative to `geometry`.
    clip: Vec<Rectangle<i32, Physical>>,
    /// How strongly it blurs.
    blur: BlurSettings,
}

impl BlurElement {
    /// Returns the blur beneath `surface`, whose top left corner is at
    /// `origin`, drawn at `scale`, if its program asks for one, as `blur`
    /// says; none when blur is off.
    pub(crate) fn beneath(
        surface: &WlSurface,
        origin: Point<i32, Physical>,
        scale: Scale<f64>,
        blur: BlurSettings,
    ) -> Option<Self> {
        if !blur.enabled {
            return None;
        }
        let (region, size) = with_states(surface, |states| {
            let region = states
                .cached_state
                .get::<BackgroundEffectSurfaceCachedState>()
                .current()
                .blur_region
                .clone();
            let size = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|state| state.lock().ok()?.surface_size());
            (region, size)
        });
        let parts: Vec<Rectangle<i32, Physical>> = region_parts(&region?, size?)
            .into_iter()
            .map(|part| {
                let mut part = part.to_f64().to_physical_precise_round(scale);
                part.loc += origin;
                part
            })
            .collect();
        let geometry = parts
            .iter()
            .copied()
            .reduce(Rectangle::merge)
            .filter(|bounds| !bounds.is_empty())?;
        let clip: Vec<_> = parts
            .into_iter()
            .map(|part| Rectangle::new(part.loc - geometry.loc, part.size))
            .collect();
        let mut hasher = std::hash::DefaultHasher::new();
        let numbers = |rectangle: &Rectangle<i32, Physical>| {
            [
                rectangle.loc.x,
                rectangle.loc.y,
                rectangle.size.w,
                rectangle.size.h,
            ]
        };
        clip.iter()
            .map(numbers)
            .collect::<Vec<_>>()
            .hash(&mut hasher);
        (geometry.size.w, geometry.size.h, blur).hash(&mut hasher);
        // Only the low bits: the namespace is a usize.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a hash cut short tells shapes apart as well"
        )]
        let namespace = hasher.finish() as usize;
        Some(Self {
            id: Id::from_wayland_resource(surface).namespaced(namespace),
            geometry,
            clip,
            blur,
        })
    }

    /// Returns the parts of `damage` (relative to the element) that ask for
    /// blur.
    fn within(&self, damage: &[Rectangle<i32, Physical>]) -> Vec<Rectangle<i32, Physical>> {
        damage
            .iter()
            .flat_map(|damaged| {
                self.clip
                    .iter()
                    .filter_map(|part| part.intersection(*damaged))
            })
            .collect()
    }
}

/// Returns the rectangles of `region` (surface-local) within a surface of
/// `size`: those added, less those taken away after them.
fn region_parts(
    region: &RegionAttributes,
    size: Size<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    let surface = Rectangle::from_size(size);
    let mut parts: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (kind, rectangle) in &region.rects {
        match kind {
            RectangleKind::Add => parts.extend(rectangle.intersection(surface)),
            RectangleKind::Subtract => {
                parts = parts
                    .into_iter()
                    .flat_map(|part| part.subtract_rect(*rectangle))
                    .collect();
            }
        }
    }
    parts
}

impl Element for BlurElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        // A new shape is a new id; nothing else changes it.
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(
            (
                f64::from(self.geometry.size.w),
                f64::from(self.geometry.size.h),
            )
                .into(),
        )
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl<R: BlurRenderer> RenderElement<R> for BlurElement {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        let Some(cache) = cache else {
            return Ok(());
        };
        let damage = self.within(damage);
        if damage.is_empty() {
            return Ok(());
        }
        R::draw_blur(frame, dst, &damage, cache)
    }

    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        R::capture_blur(frame, dst, self.blur, cache)
    }
}

impl BlurRenderer for GlesRenderer {
    fn capture_blur(
        frame: &mut GlesFrame<'_, '_>,
        area: Rectangle<i32, Physical>,
        blur: BlurSettings,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        capture_on_gpu(frame, area, blur, cache)
    }

    fn draw_blur(
        frame: &mut GlesFrame<'_, '_>,
        area: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        draw_on_gpu(frame, area, damage, cache)
    }
}

impl BlurRenderer for DrmRenderer<'_> {
    fn capture_blur(
        frame: &mut Self::Frame<'_, '_>,
        area: Rectangle<i32, Physical>,
        blur: BlurSettings,
        cache: &UserDataMap,
    ) -> Result<(), Self::Error> {
        // The frame is drawn on one GPU, with OpenGL ES.
        capture_on_gpu(frame.as_mut(), area, blur, cache)
            .map_err(smithay::backend::renderer::multigpu::Error::Render)
    }

    fn draw_blur(
        frame: &mut Self::Frame<'_, '_>,
        area: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        cache: &UserDataMap,
    ) -> Result<(), Self::Error> {
        draw_on_gpu(frame.as_mut(), area, damage, cache)
            .map_err(smithay::backend::renderer::multigpu::Error::Render)
    }
}

impl BlurRenderer for PixmanRenderer {
    fn capture_blur(
        _frame: &mut Self::Frame<'_, '_>,
        _area: Rectangle<i32, Physical>,
        _blur: BlurSettings,
        _cache: &UserDataMap,
    ) -> Result<(), Self::Error> {
        // Pixman cannot read back what it drew: no blur.
        Ok(())
    }

    fn draw_blur(
        _frame: &mut Self::Frame<'_, '_>,
        _area: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _cache: &UserDataMap,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The two blur shaders, compiled once for each OpenGL ES context and kept
/// with it.
#[derive(Debug, Clone)]
struct BlurPrograms {
    /// Halves the image.
    down: GlesTexProgram,
    /// Grows it back.
    up: GlesTexProgram,
}

impl BlurPrograms {
    /// Returns `renderer`'s blur shaders, compiling them the first time.
    fn of(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        if let Some(programs) = renderer.egl_context().user_data().get::<Self>() {
            return Ok(programs.clone());
        }
        let uniforms = [
            UniformName::new("half_pixel", UniformType::_2f),
            UniformName::new("texel", UniformType::_2f),
        ];
        let programs = Self {
            down: renderer.compile_custom_texture_shader(DOWNSAMPLE_SHADER, &uniforms)?,
            up: renderer.compile_custom_texture_shader(UPSAMPLE_SHADER, &uniforms)?,
        };
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| programs.clone());
        Ok(programs)
    }
}

/// The textures one blur works in, kept between frames: the copy of the
/// background, which ends up blurred, and each smaller size it passes
/// through.
#[derive(Debug, Default)]
struct BlurTextures {
    /// The copy first, then each half as large.
    levels: Vec<GlesTexture>,
}

impl BlurTextures {
    /// Makes the textures for a copy `size` large blurred over `passes`,
    /// unless they are already.
    fn fit(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Buffer>,
        passes: u8,
    ) -> Result<(), GlesError> {
        let count = usize::from(passes) + 1;
        let fits = self.levels.len() == count
            && self.levels.first().is_some_and(|copy| copy.size() == size);
        if fits {
            return Ok(());
        }
        self.levels = (0..count)
            .map(|level| {
                let halved = Size::from(((size.w >> level).max(1), (size.h >> level).max(1)));
                renderer.create_buffer(Fourcc::Abgr8888, halved)
            })
            .collect::<Result<_, _>>()?;
        Ok(())
    }
}

/// Copies `area` of `frame` and blurs the copy, as `blur` says, into the
/// textures kept in `cache`.
fn capture_on_gpu(
    frame: &mut GlesFrame<'_, '_>,
    area: Rectangle<i32, Physical>,
    blur: BlurSettings,
    cache: &UserDataMap,
) -> Result<(), GlesError> {
    // Where the area is in the framebuffer, which the output's transform
    // turns: as Smithay places scan-out planes.
    let in_framebuffer = frame
        .transformation()
        .transform_rect_in(area, &frame.output_size());
    if in_framebuffer.is_empty() {
        return Ok(());
    }
    let size = Size::from((in_framebuffer.size.w, in_framebuffer.size.h));
    cache.insert_if_missing(|| RefCell::new(BlurTextures::default()));
    let Some(textures) = cache.get::<RefCell<BlurTextures>>() else {
        return Ok(());
    };
    let mut textures = textures.borrow_mut();
    let passes = blur.passes.get();
    let programs = {
        let mut guard = frame.renderer();
        let renderer = guard.as_mut();
        textures.fit(renderer, size, passes)?;
        BlurPrograms::of(renderer)?
    };
    let Some(copy) = textures.levels.first_mut() else {
        return Ok(());
    };
    let mut target = frame.renderer().as_mut().bind(copy)?;
    // One OpenGL ES context runs its commands in order: the passes after
    // read the copy without waiting on its fence, as do the frame's own
    // draws after them.
    let _copied = frame.blit_to(
        &mut target,
        in_framebuffer,
        Rectangle::from_size(in_framebuffer.size),
        TextureFilter::Nearest,
    )?;
    drop(target);
    let spread = f32::from(blur.spread.get());
    let mut guard = frame.renderer();
    let renderer = guard.as_mut();
    let levels = &mut textures.levels;
    for level in 1..levels.len() {
        let (larger, smaller) = levels.split_at_mut(level);
        let (from, to) = (&larger[level - 1], &mut smaller[0]);
        let step = from.size();
        draw_pass(renderer, (from, to), &programs.down, step, spread)?;
    }
    for level in (1..levels.len()).rev() {
        let (larger, smaller) = levels.split_at_mut(level);
        let (from, to) = (&smaller[0], &mut larger[level - 1]);
        let step = to.size();
        draw_pass(renderer, (from, to), &programs.up, step, spread)?;
    }
    Ok(())
}

/// Draws `from` over all of `to` with `program`, its samples `spread`
/// half-pixels of a texture `step` large apart.
fn draw_pass(
    renderer: &mut GlesRenderer,
    (from, to): (&GlesTexture, &mut GlesTexture),
    program: &GlesTexProgram,
    step: Size<i32, Buffer>,
    spread: f32,
) -> Result<(), GlesError> {
    let source = from.size();
    let reciprocal = |length: i32| 1.0 / f32::from(u16::try_from(length).unwrap_or(u16::MAX));
    let half_pixel = (
        0.5 * spread * reciprocal(step.w),
        0.5 * spread * reciprocal(step.h),
    );
    let texel = (reciprocal(source.w), reciprocal(source.h));
    let size = Size::<i32, Physical>::from((to.size().w, to.size().h));
    let whole = Rectangle::from_size(size);
    let mut target = renderer.bind(to)?;
    let mut pass = renderer.render(&mut target, size, Transform::Normal)?;
    pass.render_texture_from_to(
        from,
        Rectangle::from_size(source).to_f64(),
        whole,
        &[whole],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &[
            Uniform::new("half_pixel", half_pixel),
            Uniform::new("texel", texel),
        ],
    )?;
    let _drawn = pass.finish()?;
    Ok(())
}

/// Draws the blurred copy kept in `cache` over `area` of `frame`, within
/// `damage` (relative to `area`).
fn draw_on_gpu(
    frame: &mut GlesFrame<'_, '_>,
    area: Rectangle<i32, Physical>,
    damage: &[Rectangle<i32, Physical>],
    cache: &UserDataMap,
) -> Result<(), GlesError> {
    let Some(textures) = cache.get::<RefCell<BlurTextures>>() else {
        return Ok(());
    };
    let textures = textures.borrow();
    let Some(blurred) = textures.levels.first() else {
        return Ok(());
    };
    // The copy is the framebuffer's way round; turn it back.
    let turned = frame.transformation().invert();
    frame.render_texture_from_to(
        blurred,
        Rectangle::from_size(blurred.size()).to_f64(),
        area,
        damage,
        &[],
        turned,
        1.0,
        None,
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rectangle at `(left, top)`, `width` by `height`.
    fn rectangle(left: i32, top: i32, width: i32, height: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((left, top).into(), (width, height).into())
    }

    /// A region of `rects`.
    fn region(rects: &[(RectangleKind, Rectangle<i32, Logical>)]) -> RegionAttributes {
        RegionAttributes {
            rects: rects.to_vec(),
        }
    }

    #[test]
    fn a_region_is_what_is_added_less_what_is_taken_away_within_the_surface() {
        let parts = region_parts(
            &region(&[
                (RectangleKind::Add, rectangle(0, 0, 1000, 1000)),
                (RectangleKind::Subtract, rectangle(0, 0, 100, 50)),
            ]),
            (100, 100).into(),
        );
        let area: i32 = parts.iter().map(|part| part.size.w * part.size.h).sum();
        assert_eq!(area, 100 * 100 - 100 * 50, "the surface's lower half");
        assert!(parts.iter().all(|part| part.loc.y >= 50));
    }

    #[test]
    fn nothing_added_is_no_region() {
        let parts = region_parts(
            &region(&[(RectangleKind::Subtract, rectangle(0, 0, 10, 10))]),
            (100, 100).into(),
        );
        assert_eq!(parts, []);
    }
}
