//! Drawing an element in its output's colours: a surface's HDR pixels on
//! an SDR monitor, or anything sRGB on a monitor in HDR
//! ([`crate::color::Conversion`]).
//!
//! A [`ColorElement`] wraps every element an output draws. Most need no
//! conversion (sRGB on an SDR output) and draw as they are, kept on a
//! plane for direct scan-out when they can be. One that does is drawn
//! through vaht's conversion shader, which OpenGL ES uses for its texture
//! draws while it draws ([`ColorRenderer`]), and never put on a plane,
//! which would show its pixels unconverted. Solid colours, drawn without a
//! texture, are converted when the scene is made instead.

use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
    UniformValue,
};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Color32F, FrameContext as _, Renderer};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

use crate::backend::DrmRenderer;
use crate::color::{Conversion, TransferFunction};

/// The conversion shader.
const SHADER: &str = include_str!("color_convert.frag");

/// A renderer that can draw texture elements converted between
/// descriptions.
pub(crate) trait ColorRenderer: Renderer {
    /// Has `frame` convert its texture draws as `conversion` says, until
    /// [`Self::stop_converting`].
    ///
    /// # Errors
    ///
    /// Returns the renderer's error if the conversion could not be set up.
    fn start_converting(
        frame: &mut Self::Frame<'_, '_>,
        conversion: &Conversion,
    ) -> Result<(), Self::Error>;

    /// Has `frame` draw textures as they are again.
    fn stop_converting(frame: &mut Self::Frame<'_, '_>);
}

/// An element drawn in its output's colours.
#[derive(Debug)]
pub(crate) struct ColorElement<E> {
    /// What it draws.
    inner: E,
    /// How its pixels become the output's, when they need to.
    conversion: Option<Conversion>,
}

impl<E> ColorElement<E> {
    /// `inner`, converted as `conversion` says unless it changes nothing.
    pub(crate) fn new(inner: E, conversion: &Conversion) -> Self {
        let conversion = (!conversion.is_identity()).then_some(*conversion);
        Self { inner, conversion }
    }

    /// `inner`, drawn as it is.
    pub(crate) const fn unconverted(inner: E) -> Self {
        Self {
            inner,
            conversion: None,
        }
    }
}

impl<E: Element> Element for ColorElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.inner.is_framebuffer_effect()
    }
}

impl<R: ColorRenderer, E: RenderElement<R>> RenderElement<R> for ColorElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        match &self.conversion {
            None => self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache),
            Some(conversion) => {
                R::start_converting(frame, conversion)?;
                let drawn = self
                    .inner
                    .draw(frame, src, dst, damage, opaque_regions, cache);
                R::stop_converting(frame);
                drawn
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        // A plane would show the pixels unconverted.
        if self.conversion.is_some() {
            None
        } else {
            self.inner.underlying_storage(renderer)
        }
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }
}

impl ColorRenderer for GlesRenderer {
    fn start_converting(
        frame: &mut GlesFrame<'_, '_>,
        conversion: &Conversion,
    ) -> Result<(), GlesError> {
        convert_on_gpu(frame, conversion)
    }

    fn stop_converting(frame: &mut GlesFrame<'_, '_>) {
        frame.clear_tex_program_override();
    }
}

impl ColorRenderer for DrmRenderer<'_> {
    fn start_converting(
        frame: &mut Self::Frame<'_, '_>,
        conversion: &Conversion,
    ) -> Result<(), Self::Error> {
        // The frame is drawn on one GPU, with OpenGL ES: its texture draws
        // go through the GPU's frame, which keeps the shader meanwhile.
        convert_on_gpu(frame.as_mut(), conversion)
            .map_err(smithay::backend::renderer::multigpu::Error::Render)
    }

    fn stop_converting(frame: &mut Self::Frame<'_, '_>) {
        frame.as_mut().clear_tex_program_override();
    }
}

impl ColorRenderer for PixmanRenderer {
    fn start_converting(
        _frame: &mut Self::Frame<'_, '_>,
        _conversion: &Conversion,
    ) -> Result<(), Self::Error> {
        // Pixman has no shaders: the headless backend's outputs are never
        // in HDR, and HDR content shows unconverted.
        Ok(())
    }

    fn stop_converting(_frame: &mut Self::Frame<'_, '_>) {}
}

/// Returns `solid` with its colour converted as `conversion` says, at
/// `scale`: solid colours are drawn without a texture, so the shader
/// cannot convert them.
pub(crate) fn convert_solid(
    solid: &SolidColorRenderElement,
    conversion: &Conversion,
    scale: Scale<f64>,
) -> SolidColorRenderElement {
    let [red, green, blue, opacity] = solid.color().components();
    // Converted unpremultiplied, then premultiplied again.
    let straight = |channel: f32| {
        if opacity > 0.0 {
            f64::from(channel / opacity)
        } else {
            0.0
        }
    };
    let converted = conversion.apply([straight(red), straight(green), straight(blue)]);
    let premultiplied = |channel: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a colour's precision is f32's"
        )]
        let channel = channel as f32;
        channel * opacity
    };
    SolidColorRenderElement::new(
        solid.id().clone(),
        solid.geometry(scale),
        solid.current_commit(),
        Color32F::new(
            premultiplied(converted[0]),
            premultiplied(converted[1]),
            premultiplied(converted[2]),
            opacity,
        ),
        solid.kind(),
    )
}

/// The conversion shader, compiled once for each OpenGL ES context and
/// kept with it.
#[derive(Debug, Clone)]
struct ColorProgram(GlesTexProgram);

impl ColorProgram {
    /// Returns `renderer`'s conversion shader, compiling it the first time.
    fn of(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        if let Some(program) = renderer.egl_context().user_data().get::<Self>() {
            return Ok(program.clone());
        }
        let uniforms = [
            UniformName::new("source_transfer", UniformType::_1i),
            UniformName::new("source_exponent", UniformType::_1f),
            UniformName::new("source_scale", UniformType::_1f),
            UniformName::new("gamut", UniformType::Matrix3x3),
            UniformName::new("tone", UniformType::_4f),
            UniformName::new("target_transfer", UniformType::_1i),
            UniformName::new("target_exponent", UniformType::_1f),
            UniformName::new("target_scale", UniformType::_1f),
        ];
        let program = Self(renderer.compile_custom_texture_shader(SHADER, &uniforms)?);
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| program.clone());
        Ok(program)
    }
}

/// Has `frame` draw textures through the conversion shader, set for
/// `conversion`, until told otherwise.
fn convert_on_gpu(frame: &mut GlesFrame<'_, '_>, conversion: &Conversion) -> Result<(), GlesError> {
    let program = ColorProgram::of(frame.renderer().as_mut())?;
    let single = |value: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a shader's precision is f32's"
        )]
        let value = value as f32;
        value
    };
    let (source_transfer, source_exponent) = transfer_uniforms(conversion.source.transfer);
    let (target_transfer, target_exponent) = transfer_uniforms(conversion.target.transfer);
    let tone = (
        if conversion.tone_maps() { 1.0 } else { 0.0 },
        single(conversion.knee),
        single(conversion.source_peak),
        single(conversion.target_peak),
    );
    frame.override_default_tex_program(
        program.0,
        vec![
            Uniform::new("source_transfer", source_transfer),
            Uniform::new("source_exponent", source_exponent),
            Uniform::new("source_scale", single(conversion.source_scale())),
            Uniform::new(
                "gamut",
                UniformValue::Matrix3x3 {
                    matrices: vec![conversion.gamut.column_major()],
                    transpose: false,
                },
            ),
            Uniform::new("tone", tone),
            Uniform::new("target_transfer", target_transfer),
            Uniform::new("target_exponent", target_exponent),
            Uniform::new("target_scale", single(conversion.target_scale())),
        ],
    );
    Ok(())
}

/// Returns the shader's number for `transfer`, and its exponent.
fn transfer_uniforms(transfer: TransferFunction) -> (i32, f32) {
    match transfer {
        TransferFunction::Srgb => (0, 1.0),
        TransferFunction::Power(exponent) => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a shader's precision is f32's"
            )]
            let exponent = exponent as f32;
            (1, exponent)
        }
        TransferFunction::Linear => (2, 1.0),
        TransferFunction::Pq => (3, 1.0),
    }
}
