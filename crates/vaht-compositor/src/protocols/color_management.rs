//! `wp_color_manager_v1`: programs saying what their pixels mean (HDR
//! video and games: BT.2100 PQ) and learning what each output shows
//! ([`crate::color`]).
//!
//! vaht describes colour parametrically: named or given primaries, a
//! named or power transfer function, luminances and mastering metadata; and
//! Windows-scRGB, which games' Vulkan drivers use. ICC profiles are not
//! taken. A surface's description is double-buffered with its commits; the
//! scene converts its pixels into its output's. A surface's feedback
//! object is told the description of the output it shows on, the one it
//! should draw in, whenever that changes.
//!
//! Smithay has no colour management yet (Smithay#1143 is a draft of these
//! protocols); `tools/smithay-watch` says when it does.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1::{self, WpColorManagementOutputV1},
    wp_color_management_surface_feedback_v1::{self, WpColorManagementSurfaceFeedbackV1},
    wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
    wp_color_manager_v1::{self, WpColorManagerV1},
    wp_image_description_creator_icc_v1::{self, WpImageDescriptionCreatorIccV1},
    wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
    wp_image_description_info_v1::WpImageDescriptionInfoV1,
    wp_image_description_v1::{self, WpImageDescriptionV1},
};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};
use smithay::wayland::compositor::{Cacheable, SurfaceData, with_states};

use crate::Compositor;
use crate::color::{Chromaticity, ImageDescription, Luminances, Primaries, TransferFunction};

/// The protocol version vaht speaks.
const VERSION: u32 = 1;

/// The transfer functions vaht converts, by name.
const TRANSFER_FUNCTIONS: [wp_color_manager_v1::TransferFunction; 6] = [
    wp_color_manager_v1::TransferFunction::Srgb,
    wp_color_manager_v1::TransferFunction::Gamma22,
    wp_color_manager_v1::TransferFunction::Gamma28,
    wp_color_manager_v1::TransferFunction::Bt1886,
    wp_color_manager_v1::TransferFunction::ExtLinear,
    wp_color_manager_v1::TransferFunction::St2084Pq,
];

/// The primaries vaht knows by name.
const NAMED_PRIMARIES: [wp_color_manager_v1::Primaries; 9] = [
    wp_color_manager_v1::Primaries::Srgb,
    wp_color_manager_v1::Primaries::PalM,
    wp_color_manager_v1::Primaries::Pal,
    wp_color_manager_v1::Primaries::Ntsc,
    wp_color_manager_v1::Primaries::GenericFilm,
    wp_color_manager_v1::Primaries::Bt2020,
    wp_color_manager_v1::Primaries::DciP3,
    wp_color_manager_v1::Primaries::DisplayP3,
    wp_color_manager_v1::Primaries::AdobeRgb,
];

/// What vaht can do, told to each program that binds the manager.
const FEATURES: [wp_color_manager_v1::Feature; 6] = [
    wp_color_manager_v1::Feature::Parametric,
    wp_color_manager_v1::Feature::SetPrimaries,
    wp_color_manager_v1::Feature::SetTfPower,
    wp_color_manager_v1::Feature::SetLuminances,
    wp_color_manager_v1::Feature::SetMasteringDisplayPrimaries,
    wp_color_manager_v1::Feature::WindowsScrgb,
];

/// The colour manager, and the outputs' colour objects, told when an
/// output's description changes.
#[derive(Debug, Default)]
pub(crate) struct ColorManagement {
    /// Every `wp_color_management_output_v1`.
    outputs: Vec<WpColorManagementOutputV1>,
    /// Information objects asked for, and what to tell them, once the
    /// requests that made them are dispatched.
    undescribed: Vec<(WpImageDescriptionInfoV1, ImageDescription)>,
}

impl ColorManagement {
    /// Offers the manager on `display`.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        display.create_global::<Compositor, WpColorManagerV1, _>(VERSION, ());
        Self::default()
    }

    /// Tells each information object asked for what its description is:
    /// after the clients' requests are dispatched, since that destroys it.
    pub(crate) fn describe_asked(&mut self) {
        for (info, description) in self.undescribed.drain(..) {
            if info.is_alive() {
                describe(&info, &description);
            }
        }
    }

    /// Tells programs watching `output` that what it shows changed (it
    /// went into or out of HDR).
    pub(crate) fn output_changed(&self, output: &Output) {
        for object in &self.outputs {
            if object
                .data::<OutputData>()
                .and_then(|data| data.output.upgrade().ok())
                .is_some_and(|watched| Output::from_resource(&watched).as_ref() == Some(output))
            {
                object.image_description_changed();
            }
        }
    }
}

/// A surface's description, double-buffered: none is sRGB.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SurfaceColor {
    /// What its pixels mean, if it said.
    pub(crate) description: Option<ImageDescription>,
}

impl Cacheable for SurfaceColor {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// Returns what the pixels of the surface whose `states` these are mean.
pub(crate) fn surface_description(states: &SurfaceData) -> ImageDescription {
    states
        .cached_state
        .get::<SurfaceColor>()
        .current()
        .description
        .unwrap_or(ImageDescription::SRGB)
}

/// Tells the feedback objects of the surface whose `states` these are, if
/// it has any, that it shows on `output`: whose description it should
/// draw in, if that changed.
pub(crate) fn tell_preferred(states: &SurfaceData, output: &Output) {
    let Some(colors) = states.data_map.get::<SurfaceColorObjects>() else {
        return;
    };
    let description = ImageDescription::of_output(output);
    let Ok(feedbacks) = colors.feedbacks.lock() else {
        return;
    };
    for feedback in feedbacks.iter() {
        if let Some(data) = feedback.data::<FeedbackData>()
            && data.prefer(description)
        {
            feedback.preferred_changed(description.identity());
        }
    }
}

/// What a surface's colour objects are, kept with it.
#[derive(Debug, Default)]
struct SurfaceColorObjects {
    /// Whether it has a `wp_color_management_surface_v1`: one at most.
    has_surface_object: AtomicBool,
    /// Its feedback objects.
    feedbacks: Mutex<Vec<WpColorManagementSurfaceFeedbackV1>>,
}

/// What an output colour object watches.
#[derive(Debug)]
pub(crate) struct OutputData {
    /// The output.
    output: Weak<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
}

/// What a surface colour object belongs to.
#[derive(Debug)]
pub(crate) struct SurfaceObjectData {
    /// The surface.
    surface: Weak<WlSurface>,
}

/// What a feedback object belongs to, and what it last told.
#[derive(Debug)]
pub(crate) struct FeedbackData {
    /// The surface.
    surface: Weak<WlSurface>,
    /// The description it should draw in.
    preferred: Mutex<ImageDescription>,
    /// The identity last told, 0 for none.
    told: AtomicU32,
}

impl FeedbackData {
    /// Makes `description` the preferred one; returns whether that changed
    /// what was last told.
    fn prefer(&self, description: ImageDescription) -> bool {
        if let Ok(mut preferred) = self.preferred.lock() {
            *preferred = description;
        }
        let identity = description.identity();
        self.told.swap(identity, Ordering::Relaxed) != identity
    }

    /// Returns the description the surface should draw in.
    fn preferred(&self) -> ImageDescription {
        self.preferred
            .lock()
            .map_or(ImageDescription::SRGB, |preferred| *preferred)
    }
}

/// What an image description object stands for.
#[derive(Debug)]
pub(crate) struct DescriptionData {
    /// The description.
    description: ImageDescription,
    /// Whether a program may ask what it is: vaht's own (an output's, a
    /// surface's preferred), not one the program made.
    is_informative: bool,
}

/// What a parametric creator has been given so far.
#[derive(Debug, Default)]
pub(crate) struct CreatorData {
    /// The parameters.
    given: Mutex<Parameters>,
}

/// The parameters of an image description being made.
#[derive(Debug, Default, Clone, Copy)]
struct Parameters {
    /// The transfer function.
    transfer: Option<TransferFunction>,
    /// The primaries.
    primaries: Option<Primaries>,
    /// The luminances.
    luminances: Option<Luminances>,
    /// Whether mastering display primaries were given (vaht keeps no
    /// more than that they were: it maps gamut by the primaries).
    has_mastering_primaries: bool,
    /// The mastering display's black and peak.
    mastering_luminance: Option<(f64, f64)>,
    /// The brightest pixel of the content.
    max_cll: Option<f64>,
    /// The brightest frame average of the content.
    max_fall: Option<f64>,
}

impl Parameters {
    /// Returns the description made of the parameters, or none if the
    /// transfer function or primaries are missing.
    fn description(&self) -> Option<ImageDescription> {
        let (transfer, primaries) = (self.transfer?, self.primaries?);
        let defaults = match transfer {
            TransferFunction::Pq => Luminances::PQ,
            TransferFunction::Power(exponent) if (exponent - 2.4).abs() < f64::EPSILON => {
                Luminances {
                    min: 0.01,
                    max: 100.0,
                    reference: 100.0,
                }
            }
            TransferFunction::Srgb | TransferFunction::Power(_) | TransferFunction::Linear => {
                Luminances::SDR
            }
        };
        let mut luminances = self.luminances.unwrap_or(defaults);
        if transfer.is_absolute() {
            // PQ's range is fixed: 10 000 above its black.
            luminances.max = luminances.min + TransferFunction::PQ_PEAK;
        }
        let target_peak = self
            .max_cll
            .or(self.mastering_luminance.map(|(_, peak)| peak));
        let _ = (self.has_mastering_primaries, self.max_fall);
        Some(ImageDescription {
            primaries,
            transfer,
            luminances,
            target_peak,
        })
    }
}

/// Returns the transfer function `named`, if vaht converts it.
fn transfer_named(named: wp_color_manager_v1::TransferFunction) -> Option<TransferFunction> {
    match named {
        wp_color_manager_v1::TransferFunction::Srgb => Some(TransferFunction::Srgb),
        wp_color_manager_v1::TransferFunction::Gamma22 => Some(TransferFunction::Power(2.2)),
        wp_color_manager_v1::TransferFunction::Gamma28 => Some(TransferFunction::Power(2.8)),
        wp_color_manager_v1::TransferFunction::Bt1886 => Some(TransferFunction::Power(2.4)),
        wp_color_manager_v1::TransferFunction::ExtLinear => Some(TransferFunction::Linear),
        wp_color_manager_v1::TransferFunction::St2084Pq => Some(TransferFunction::Pq),
        wp_color_manager_v1::TransferFunction::St240
        | wp_color_manager_v1::TransferFunction::Log100
        | wp_color_manager_v1::TransferFunction::Log316
        | wp_color_manager_v1::TransferFunction::Xvycc
        | wp_color_manager_v1::TransferFunction::ExtSrgb
        | wp_color_manager_v1::TransferFunction::St428
        | wp_color_manager_v1::TransferFunction::Hlg
        | wp_color_manager_v1::TransferFunction::CompoundPower24
        | _ => None,
    }
}

/// Returns the primaries `named`, if vaht knows them.
fn primaries_named(named: wp_color_manager_v1::Primaries) -> Option<Primaries> {
    /// Illuminant C, the white of old television standards.
    const ILLUMINANT_C: Chromaticity = Chromaticity::new(0.310, 0.316);
    /// DCI's white, cinema projection's.
    const DCI: Chromaticity = Chromaticity::new(0.314, 0.351);
    /// D65.
    const D65: Chromaticity = Primaries::SRGB.white;
    let primaries = |red: (f64, f64), green: (f64, f64), blue: (f64, f64), white| Primaries {
        red: Chromaticity::new(red.0, red.1),
        green: Chromaticity::new(green.0, green.1),
        blue: Chromaticity::new(blue.0, blue.1),
        white,
    };
    match named {
        wp_color_manager_v1::Primaries::Srgb => Some(Primaries::SRGB),
        wp_color_manager_v1::Primaries::PalM => Some(primaries(
            (0.67, 0.33),
            (0.21, 0.71),
            (0.14, 0.08),
            ILLUMINANT_C,
        )),
        wp_color_manager_v1::Primaries::Pal => {
            Some(primaries((0.64, 0.33), (0.29, 0.60), (0.15, 0.06), D65))
        }
        wp_color_manager_v1::Primaries::Ntsc => Some(primaries(
            (0.630, 0.340),
            (0.310, 0.595),
            (0.155, 0.070),
            D65,
        )),
        wp_color_manager_v1::Primaries::GenericFilm => Some(primaries(
            (0.681, 0.319),
            (0.243, 0.692),
            (0.145, 0.049),
            ILLUMINANT_C,
        )),
        wp_color_manager_v1::Primaries::Bt2020 => Some(Primaries::BT2020),
        wp_color_manager_v1::Primaries::DciP3 => Some(primaries(
            (0.680, 0.320),
            (0.265, 0.690),
            (0.150, 0.060),
            DCI,
        )),
        wp_color_manager_v1::Primaries::DisplayP3 => Some(Primaries::DISPLAY_P3),
        wp_color_manager_v1::Primaries::AdobeRgb => {
            Some(primaries((0.64, 0.33), (0.21, 0.71), (0.15, 0.06), D65))
        }
        wp_color_manager_v1::Primaries::Cie1931Xyz | _ => None,
    }
}

/// Returns the primaries given as the protocol's millionths.
fn primaries_given(numbers: [i32; 8]) -> Primaries {
    let point = |index: usize| {
        Chromaticity::new(
            f64::from(numbers[index]) / 1_000_000.0,
            f64::from(numbers[index + 1]) / 1_000_000.0,
        )
    };
    Primaries {
        red: point(0),
        green: point(2),
        blue: point(4),
        white: point(6),
    }
}

/// Returns `primaries` as the protocol's millionths.
fn primaries_numbers(primaries: Primaries) -> [i32; 8] {
    let million = |value: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "chromaticities are between 0 and 1"
        )]
        let number = (value * 1_000_000.0).round() as i32;
        number
    };
    [
        primaries.red,
        primaries.green,
        primaries.blue,
        primaries.white,
    ]
    .iter()
    .flat_map(|point| [million(point.x), million(point.y)])
    .collect::<Vec<_>>()
    .try_into()
    .unwrap_or([0; 8])
}

/// Returns `nits` as a whole number for the protocol.
fn whole(nits: f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "luminances are positive and far below u32::MAX"
    )]
    let number = nits.max(0.0).round() as u32;
    number
}

/// Makes `id` an image description of `description` and says it is ready.
fn ready_description(
    data_init: &mut DataInit<'_, Compositor>,
    id: New<WpImageDescriptionV1>,
    description: ImageDescription,
    is_informative: bool,
) {
    let object = data_init.init(
        id,
        DescriptionData {
            description,
            is_informative,
        },
    );
    object.ready(description.identity());
}

/// Tells `info` what `description` is, then that that is all.
fn describe(info: &WpImageDescriptionInfoV1, description: &ImageDescription) {
    let [rx, ry, gx, gy, bx, by, wx, wy] = primaries_numbers(description.primaries);
    info.primaries(rx, ry, gx, gy, bx, by, wx, wy);
    if let Some(named) = NAMED_PRIMARIES
        .iter()
        .find(|named| primaries_named(**named) == Some(description.primaries))
    {
        info.primaries_named(*named);
    }
    match description.transfer {
        TransferFunction::Srgb => info.tf_named(wp_color_manager_v1::TransferFunction::Srgb),
        TransferFunction::Linear => {
            info.tf_named(wp_color_manager_v1::TransferFunction::ExtLinear);
        }
        TransferFunction::Pq => info.tf_named(wp_color_manager_v1::TransferFunction::St2084Pq),
        TransferFunction::Power(exponent) => info.tf_power(whole(exponent * 10_000.0)),
    }
    let luminances = description.luminances;
    let min = whole(luminances.min * 10_000.0);
    info.luminances(min, whole(luminances.max), whole(luminances.reference));
    info.target_primaries(rx, ry, gx, gy, bx, by, wx, wy);
    let peak = description.target_peak.unwrap_or(luminances.max);
    info.target_luminance(min, whole(peak));
    info.done();
}

impl GlobalDispatch<WpColorManagerV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<WpColorManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        manager.supported_intent(wp_color_manager_v1::RenderIntent::Perceptual);
        for feature in FEATURES {
            manager.supported_feature(feature);
        }
        for transfer in TRANSFER_FUNCTIONS {
            manager.supported_tf_named(transfer);
        }
        for primaries in NAMED_PRIMARIES {
            manager.supported_primaries_named(primaries);
        }
        manager.done();
    }
}

impl Dispatch<WpColorManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_color_manager_v1::Request::GetOutput { id, output } => {
                let object = data_init.init(
                    id,
                    OutputData {
                        output: output.downgrade(),
                    },
                );
                state.protocols.color_management.outputs.push(object);
            }
            wp_color_manager_v1::Request::GetSurface { id, surface } => {
                let objects = with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(SurfaceColorObjects::default);
                    states
                        .data_map
                        .get::<SurfaceColorObjects>()
                        .is_some_and(|objects| {
                            objects.has_surface_object.swap(true, Ordering::Relaxed)
                        })
                });
                if objects {
                    resource.post_error(
                        wp_color_manager_v1::Error::SurfaceExists,
                        "the surface already has a colour management object",
                    );
                    return;
                }
                data_init.init(
                    id,
                    SurfaceObjectData {
                        surface: surface.downgrade(),
                    },
                );
            }
            wp_color_manager_v1::Request::GetSurfaceFeedback { id, surface } => {
                let description = ImageDescription::of_output(state.outputs.primary());
                let feedback = data_init.init(
                    id,
                    FeedbackData {
                        surface: surface.downgrade(),
                        preferred: Mutex::new(description),
                        told: AtomicU32::new(description.identity()),
                    },
                );
                with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(SurfaceColorObjects::default);
                    if let Some(objects) = states.data_map.get::<SurfaceColorObjects>()
                        && let Ok(mut feedbacks) = objects.feedbacks.lock()
                    {
                        feedbacks.push(feedback);
                    }
                });
            }
            wp_color_manager_v1::Request::CreateIccCreator { obj } => {
                data_init.init(obj, ());
                resource.post_error(
                    wp_color_manager_v1::Error::UnsupportedFeature,
                    "vaht takes no ICC profiles",
                );
            }
            wp_color_manager_v1::Request::CreateParametricCreator { obj } => {
                data_init.init(obj, CreatorData::default());
            }
            wp_color_manager_v1::Request::CreateWindowsScrgb { image_description } => {
                // sRGB primaries, linear, 1.0 at 80 cd/m², reaching 10 000;
                // reference white assumed at 203 (BT.2408).
                let scrgb = ImageDescription {
                    primaries: Primaries::SRGB,
                    transfer: TransferFunction::Linear,
                    luminances: Luminances {
                        min: 0.0,
                        max: 80.0,
                        reference: Luminances::PQ.reference,
                    },
                    target_peak: Some(TransferFunction::PQ_PEAK),
                };
                ready_description(data_init, image_description, scrgb, false);
            }
            wp_color_manager_v1::Request::Destroy | _ => {}
        }
    }
}

impl Dispatch<WpColorManagementOutputV1, OutputData> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        data: &OutputData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_color_management_output_v1::Request::GetImageDescription { image_description } =
            request
        {
            let description = data
                .output
                .upgrade()
                .ok()
                .and_then(|output| Output::from_resource(&output))
                .map_or(ImageDescription::SRGB, |output| {
                    ImageDescription::of_output(&output)
                });
            ready_description(data_init, image_description, description, true);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &WpColorManagementOutputV1,
        _data: &OutputData,
    ) {
        state
            .protocols
            .color_management
            .outputs
            .retain(|output| output != resource);
    }
}

impl Dispatch<WpColorManagementSurfaceV1, SurfaceObjectData> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        data: &SurfaceObjectData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Ok(surface) = data.surface.upgrade() else {
            if !matches!(request, wp_color_management_surface_v1::Request::Destroy) {
                resource.post_error(
                    wp_color_management_surface_v1::Error::Inert,
                    "the surface was destroyed",
                );
            }
            return;
        };
        let description = match request {
            wp_color_management_surface_v1::Request::SetImageDescription {
                image_description,
                render_intent,
            } => {
                if render_intent.into_result().ok()
                    != Some(wp_color_manager_v1::RenderIntent::Perceptual)
                {
                    resource.post_error(
                        wp_color_management_surface_v1::Error::RenderIntent,
                        "vaht renders perceptually only",
                    );
                    return;
                }
                let Some(given) = image_description.data::<DescriptionData>() else {
                    resource.post_error(
                        wp_color_management_surface_v1::Error::ImageDescription,
                        "the image description is not ready",
                    );
                    return;
                };
                Some(given.description)
            }
            wp_color_management_surface_v1::Request::UnsetImageDescription
            | wp_color_management_surface_v1::Request::Destroy => None,
            _ => return,
        };
        with_states(&surface, |states| {
            states
                .cached_state
                .get::<SurfaceColor>()
                .pending()
                .description = description;
        });
    }

    fn destroyed(
        _state: &mut Self,
        _client: ClientId,
        _resource: &WpColorManagementSurfaceV1,
        data: &SurfaceObjectData,
    ) {
        if let Ok(surface) = data.surface.upgrade() {
            with_states(&surface, |states| {
                if let Some(objects) = states.data_map.get::<SurfaceColorObjects>() {
                    objects.has_surface_object.store(false, Ordering::Relaxed);
                }
            });
        }
    }
}

impl Dispatch<WpColorManagementSurfaceFeedbackV1, FeedbackData> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        data: &FeedbackData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let id = match request {
            wp_color_management_surface_feedback_v1::Request::GetPreferred {
                image_description,
            }
            | wp_color_management_surface_feedback_v1::Request::GetPreferredParametric {
                image_description,
            } => image_description,
            wp_color_management_surface_feedback_v1::Request::Destroy | _ => return,
        };
        if data.surface.upgrade().is_err() {
            data_init.init(
                id,
                DescriptionData {
                    description: ImageDescription::SRGB,
                    is_informative: false,
                },
            );
            resource.post_error(
                wp_color_management_surface_feedback_v1::Error::Inert,
                "the surface was destroyed",
            );
            return;
        }
        ready_description(data_init, id, data.preferred(), true);
    }

    fn destroyed(
        _state: &mut Self,
        _client: ClientId,
        resource: &WpColorManagementSurfaceFeedbackV1,
        data: &FeedbackData,
    ) {
        if let Ok(surface) = data.surface.upgrade() {
            with_states(&surface, |states| {
                if let Some(objects) = states.data_map.get::<SurfaceColorObjects>()
                    && let Ok(mut feedbacks) = objects.feedbacks.lock()
                {
                    feedbacks.retain(|feedback| feedback != resource);
                }
            });
        }
    }
}

impl Dispatch<WpImageDescriptionCreatorIccV1, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &WpImageDescriptionCreatorIccV1,
        _request: wp_image_description_creator_icc_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Never made: asking for one is a protocol error already.
        resource.post_error(
            wp_image_description_creator_icc_v1::Error::IncompleteSet,
            "vaht takes no ICC profiles",
        );
    }
}

impl Dispatch<WpImageDescriptionCreatorParamsV1, CreatorData> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        data: &CreatorData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let Ok(mut given) = data.given.lock() else {
            return;
        };
        let wp_image_description_creator_params_v1::Request::Create { image_description } = request
        else {
            if let Err((error, message)) = given.set(&request) {
                resource.post_error(error, message);
            }
            return;
        };
        if let Some(description) = given.description() {
            ready_description(data_init, image_description, description, false);
        } else {
            data_init.init(
                image_description,
                DescriptionData {
                    description: ImageDescription::SRGB,
                    is_informative: false,
                },
            );
            resource.post_error(
                wp_image_description_creator_params_v1::Error::IncompleteSet,
                "a transfer function and primaries are needed",
            );
        }
    }
}

impl Parameters {
    /// Takes the parameter `request` gives, or says why not: the protocol
    /// error to raise.
    fn set(
        &mut self,
        request: &wp_image_description_creator_params_v1::Request,
    ) -> Result<(), (wp_image_description_creator_params_v1::Error, &'static str)> {
        use wp_image_description_creator_params_v1::{Error, Request};
        /// Fails with `already_set` if `slot` is.
        fn unset<T>(slot: Option<T>, what: &'static str) -> Result<(), (Error, &'static str)> {
            slot.map_or(Ok(()), |_| Err((Error::AlreadySet, what)))
        }
        match request {
            &Request::SetTfNamed { tf } => {
                unset(self.transfer, "the transfer function is set")?;
                let transfer = tf.into_result().ok().and_then(transfer_named);
                self.transfer = Some(
                    transfer.ok_or((Error::InvalidTf, "vaht does not convert that function"))?,
                );
            }
            &Request::SetTfPower { eexp } => {
                unset(self.transfer, "the transfer function is set")?;
                if !(10_000..=100_000).contains(&eexp) {
                    return Err((Error::InvalidTf, "the exponent is 1 to 10"));
                }
                self.transfer = Some(TransferFunction::Power(f64::from(eexp) / 10_000.0));
            }
            &Request::SetPrimariesNamed { primaries } => {
                unset(self.primaries, "the primaries are set")?;
                let named = primaries.into_result().ok().and_then(primaries_named);
                self.primaries =
                    Some(named.ok_or((Error::InvalidPrimariesNamed, "vaht does not know them"))?);
            }
            &Request::SetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                unset(self.primaries, "the primaries are set")?;
                self.primaries = Some(primaries_given([r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y]));
            }
            &Request::SetLuminances {
                min_lum,
                max_lum,
                reference_lum,
            } => {
                unset(self.luminances, "the luminances are set")?;
                let min = f64::from(min_lum) / 10_000.0;
                let (max, reference) = (f64::from(max_lum), f64::from(reference_lum));
                if max <= min || reference <= min {
                    return Err((
                        Error::InvalidLuminance,
                        "the peak and white must exceed black",
                    ));
                }
                self.luminances = Some(Luminances {
                    min,
                    max,
                    reference,
                });
            }
            Request::SetMasteringDisplayPrimaries { .. } => {
                if self.has_mastering_primaries {
                    return Err((Error::AlreadySet, "the mastering primaries are set"));
                }
                self.has_mastering_primaries = true;
            }
            &Request::SetMasteringLuminance { min_lum, max_lum } => {
                unset(self.mastering_luminance, "the mastering luminance is set")?;
                let (min, max) = (f64::from(min_lum) / 10_000.0, f64::from(max_lum));
                if max <= min {
                    return Err((Error::InvalidLuminance, "the peak must exceed black"));
                }
                self.mastering_luminance = Some((min, max));
            }
            &Request::SetMaxCll { max_cll } => {
                unset(self.max_cll, "max CLL is set")?;
                self.max_cll = Some(f64::from(max_cll));
            }
            &Request::SetMaxFall { max_fall } => {
                unset(self.max_fall, "max FALL is set")?;
                self.max_fall = Some(f64::from(max_fall));
            }
            Request::Create { .. } | _ => {}
        }
        Ok(())
    }
}

impl Dispatch<WpImageDescriptionV1, DescriptionData> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        data: &DescriptionData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_image_description_v1::Request::GetInformation { information } = request {
            let info = data_init.init(information, ());
            if data.is_informative {
                // `done` destroys the object, which the server cannot do
                // while still dispatching the request that made it.
                state
                    .protocols
                    .color_management
                    .undescribed
                    .push((info, data.description));
            } else {
                resource.post_error(
                    wp_image_description_v1::Error::NoInformation,
                    "a program's own description is not described back",
                );
            }
        }
    }
}

impl Dispatch<WpImageDescriptionInfoV1, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &WpImageDescriptionInfoV1,
        _request: <WpImageDescriptionInfoV1 as Resource>::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // It has no requests.
    }
}
