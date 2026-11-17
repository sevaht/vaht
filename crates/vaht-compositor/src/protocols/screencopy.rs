//! `zwlr_screencopy_manager_v1`: screen capture as older tools ask for it
//! (wf-recorder, OBS's wlrobs, grim before 1.5), beside
//! `ext-image-copy-capture`, which newer ones use.
//!
//! A frame offers a shared-memory format and, for a whole output on the
//! hardware, a GPU buffer format; the client copies into a buffer of its
//! own. `copy` draws at once; `copy_with_damage`, which recorders loop on,
//! waits until vaht next draws, so they record at vaht's pace rather than
//! as fast as they can ask. A region is cut from the whole output's image,
//! so it is offered in shared memory only.

use std::sync::atomic::{AtomicBool, Ordering};

use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1};
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource as _,
};
use smithay::utils::{Physical, Rectangle, Size, Transform};

use crate::Compositor;
use crate::capture::{CaptureError, CaptureView, CapturedImage};
use crate::client::ClientState;

/// The bytes of one pixel, `XRGB8888`.
const BYTES_PER_PIXEL: i32 = 4;

/// The frames waiting to be copied the next time vaht draws.
#[derive(Debug, Default)]
pub(crate) struct Screencopies {
    /// Each frame, with the client's buffer to copy into.
    waiting: Vec<(ZwlrScreencopyFrameV1, WlBuffer)>,
}

impl Screencopies {
    /// The protocol version vaht speaks.
    const VERSION: u32 = 3;

    /// Offers the manager on `display`, to unsandboxed clients.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        display.create_global::<Compositor, ZwlrScreencopyManagerV1, _>(Self::VERSION, ());
        Self::default()
    }
}

/// What a frame captures.
#[derive(Debug)]
pub(crate) struct FrameData {
    /// The output.
    output: WeakOutput,
    /// The part of the output's image, if not all of it.
    region: Option<Rectangle<i32, Physical>>,
    /// How large the image is.
    size: Size<i32, Physical>,
    /// Whether the pointer is drawn in it.
    draws_pointer: bool,
    /// Whether the client copied it already: a frame is copied once.
    is_used: AtomicBool,
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Compositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, (): &()) -> bool {
        ClientState::is_unrestricted(&client)
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        (): &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, overlay_cursor, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (
                frame,
                overlay_cursor,
                output,
                Some(Rectangle::new((x, y).into(), (width, height).into())),
            ),
            zwlr_screencopy_manager_v1::Request::Destroy | _ => return,
        };
        let Some((output_object, region, size)) = frame_extent(&output, region) else {
            let frame = data_init.init(
                frame,
                FrameData {
                    output: WeakOutput::default(),
                    region: None,
                    size: Size::default(),
                    draws_pointer: false,
                    is_used: AtomicBool::new(true),
                },
            );
            frame.failed();
            return;
        };
        let frame = data_init.init(
            frame,
            FrameData {
                output: output_object.downgrade(),
                region,
                size,
                draws_pointer: overlay_cursor != 0,
                is_used: AtomicBool::new(false),
            },
        );
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            unsigned(size.w),
            unsigned(size.h),
            unsigned(size.w * BYTES_PER_PIXEL),
        );
        if frame.version() >= 3 {
            // A GPU buffer for a whole output, which the hardware draws
            // into straight.
            if region.is_none()
                && let Some(constraints) = state.backend.capture_dmabuf_constraints()
                && let Some((format, _)) = constraints.formats.first()
            {
                frame.linux_dmabuf(*format as u32, unsigned(size.w), unsigned(size.h));
            }
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, waits_for_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy | _ => return,
        };
        if data.is_used.swap(true, Ordering::Relaxed) {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "the frame was copied already",
            );
            return;
        }
        if waits_for_damage {
            state
                .protocols
                .screencopies
                .waiting
                .push((resource.clone(), buffer));
            state.schedule_render();
        } else {
            state.copy_screen(resource, &buffer, false);
        }
    }
}

impl Compositor {
    /// Copies the frames waiting for vaht to draw: it just drew.
    pub(crate) fn copy_waiting_screens(&mut self) {
        let waiting = std::mem::take(&mut self.protocols.screencopies.waiting);
        for (frame, buffer) in waiting {
            if frame.is_alive() {
                self.copy_screen(&frame, &buffer, true);
            }
        }
    }

    /// Draws `frame`'s output into the client's `buffer`, and tells it the
    /// image is ready (with the damage, if `reports_damage`) or failed.
    fn copy_screen(
        &mut self,
        frame: &ZwlrScreencopyFrameV1,
        buffer: &WlBuffer,
        reports_damage: bool,
    ) {
        let Some(data) = frame.data::<FrameData>() else {
            return;
        };
        let Some(output) = data.output.upgrade() else {
            frame.failed();
            return;
        };
        let view = CaptureView::Output(output);
        let drawn = if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(buffer) {
            if data.region.is_some() {
                Err(CaptureError::Draw(
                    "a region is copied to shared memory only".to_owned(),
                ))
            } else {
                let mut dmabuf = dmabuf.clone();
                self.with_capture_scene(data.draws_pointer, |backend, scene| {
                    backend.capture_into(scene, &view, &mut dmabuf)
                })
            }
        } else {
            self.with_capture_scene(data.draws_pointer, |backend, scene| {
                backend.capture(scene, &view)
            })
            .and_then(|image| crop(image, data.region))
            .and_then(|image| {
                crate::shared_memory::write_frame(buffer, &image.pixels, image.width, image.height)
                    .map_err(CaptureError::Write)
            })
        };
        match drawn {
            Ok(()) => {
                frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                if reports_damage && frame.version() >= 2 {
                    frame.damage(0, 0, unsigned(data.size.w), unsigned(data.size.h));
                }
                let time = crate::presentation::now();
                let seconds = time.as_secs();
                frame.ready(
                    u32::try_from(seconds >> 32).unwrap_or(u32::MAX),
                    u32::try_from(seconds & u64::from(u32::MAX)).unwrap_or(u32::MAX),
                    time.subsec_nanos(),
                );
            }
            Err(error) => {
                tracing::debug!(
                    error = &error as &dyn std::error::Error,
                    "a screen copy failed"
                );
                frame.failed();
            }
        }
    }
}

/// An output, the part of its image a frame covers (all of it when none),
/// and the image's size.
type FrameExtent = (
    Output,
    Option<Rectangle<i32, Physical>>,
    Size<i32, Physical>,
);

/// Returns the output `output` names, the part of its image `region`
/// (output-local, logical) covers in pixels, and the image's size; none if
/// the output is gone or has no mode, or the region is empty or turned
/// (a rotated output's region is not cut).
fn frame_extent(
    output: &WlOutput,
    region: Option<Rectangle<i32, smithay::utils::Logical>>,
) -> Option<FrameExtent> {
    let output = Output::from_resource(output)?;
    let whole = output.current_mode()?.size;
    let Some(region) = region else {
        return Some((output, None, whole));
    };
    if output.current_transform() != Transform::Normal {
        return None;
    }
    let scale = output.current_scale().fractional_scale();
    let pixels = region
        .to_f64()
        .to_physical(scale)
        .to_i32_round()
        .intersection(Rectangle::from_size(whole))?;
    (pixels.size.w > 0 && pixels.size.h > 0).then_some((output, Some(pixels), pixels.size))
}

/// Returns the part `region` of `image`, or all of it without one.
fn crop(
    image: CapturedImage,
    region: Option<Rectangle<i32, Physical>>,
) -> Result<CapturedImage, CaptureError> {
    let Some(region) = region else {
        return Ok(image);
    };
    let index = |value: i32| usize::try_from(value).unwrap_or(0);
    let (left, top) = (index(region.loc.x), index(region.loc.y));
    let (width, height) = (index(region.size.w), index(region.size.h));
    if left + width > image.width || top + height > image.height {
        return Err(CaptureError::Draw(
            "the region is outside the image".to_owned(),
        ));
    }
    let bytes = usize::try_from(BYTES_PER_PIXEL).unwrap_or(4);
    let row_length = image.width * bytes;
    let pixels = (top..top + height)
        .flat_map(|row| {
            let start = row * row_length + left * bytes;
            image.pixels[start..start + width * bytes].iter().copied()
        })
        .collect();
    Ok(CapturedImage {
        pixels,
        width,
        height,
    })
}

/// Returns `value`, a size, as the protocol's unsigned number.
fn unsigned(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_is_cut_from_the_image_row_by_row() {
        // A 3 × 2 image whose pixels are numbered 0 to 5.
        let pixels: Vec<u8> = (0..6_u8).flat_map(|pixel| [pixel; 4]).collect();
        let image = CapturedImage {
            pixels,
            width: 3,
            height: 2,
        };
        let cut = crop(image, Some(Rectangle::new((1, 0).into(), (2, 2).into()))).expect("inside");
        let numbers: Vec<u8> = cut.pixels.chunks(4).map(|pixel| pixel[0]).collect();
        assert_eq!((cut.width, cut.height), (2, 2));
        assert_eq!(numbers, [1, 2, 4, 5]);
    }

    #[test]
    fn a_region_outside_the_image_is_refused() {
        let image = CapturedImage {
            pixels: vec![0; 4],
            width: 1,
            height: 1,
        };
        assert!(crop(image, Some(Rectangle::new((0, 0).into(), (2, 1).into()))).is_err());
    }
}
