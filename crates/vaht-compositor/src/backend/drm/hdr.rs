//! Driving a monitor in HDR10: telling it the signal is BT.2020 colour
//! with PQ encoding (the connector's `Colorspace` and
//! `HDR_OUTPUT_METADATA`), so it shows the PQ values vaht draws
//! ([`crate::color`]) as the light they encode.
//!
//! A monitor can if its EDID says it takes PQ and BT.2020 RGB; how bright
//! it gets comes from the EDID too. Smithay sets neither property
//! (Smithay#982 plans HDR metadata after its colour pipeline), so vaht
//! commits them itself, on the connector alone: the CRTC and planes keep
//! what Smithay set.

use smithay::reexports::drm::control::atomic::AtomicModeReq;
use smithay::reexports::drm::control::{
    AtomicCommitFlags, Device as ControlDevice, connector, property,
};

/// What a monitor in HDR can show, from its EDID.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HdrCapability {
    /// The brightest it gets, in cd/m².
    pub(crate) peak: f64,
    /// The brightest a whole frame gets, on average, in cd/m².
    pub(crate) frame_average: f64,
    /// Its black, in cd/m².
    pub(crate) black: f64,
}

impl HdrCapability {
    /// What a monitor that gives no luminances is assumed to reach: a
    /// modest HDR monitor's.
    const ASSUMED_PEAK: f64 = 400.0;

    /// Returns what a monitor whose EDID says `takes_hdr10` (PQ in
    /// BT.2020 RGB) can show, with its desired peak, frame average and
    /// black luminances (0 when it gives none); none if it cannot.
    pub(crate) fn of(takes_hdr10: bool, [peak, frame_average, black]: [f32; 3]) -> Option<Self> {
        let known = |value: f32| (value > 0.0).then_some(f64::from(value));
        let peak = known(peak).unwrap_or(Self::ASSUMED_PEAK);
        takes_hdr10.then_some(Self {
            peak,
            frame_average: known(frame_average).unwrap_or(peak),
            black: known(black).unwrap_or(0.0),
        })
    }

    /// Returns the HDR10 static metadata for a monitor of this capability
    /// reaching `peak`: the kernel's `struct hdr_output_metadata`, its
    /// infoframe type 1 (CTA-861-G), little-endian.
    fn metadata(&self, peak: f64) -> [u8; 32] {
        /// The infoframe's EOTF: SMPTE ST 2084, PQ.
        const PQ: u8 = 2;
        /// BT.2020's primaries, red, green, blue, then white, in the
        /// infoframe's units of 0.00002.
        const BT2020: [(u16, u16); 4] =
            [(35400, 14600), (8500, 39850), (6550, 2300), (15635, 16450)];
        let whole = |value: f64| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped into u16's range first"
            )]
            let value = value.round().clamp(0.0, f64::from(u16::MAX)) as u16;
            value
        };
        let mut bytes = [0; 32];
        // metadata_type 0, a u32, then the infoframe.
        bytes[4] = PQ;
        bytes[5] = 0;
        let mut words: Vec<u16> = BT2020.iter().flat_map(|&(x, y)| [x, y]).collect();
        words.extend([
            whole(peak),
            whole(self.black * 10_000.0),
            whole(peak),
            whole(self.frame_average.min(peak)),
        ]);
        for (index, word) in words.iter().enumerate() {
            let at = 6 + index * 2;
            bytes[at..at + 2].copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}

/// Why a monitor could not be put into or out of HDR.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HdrError {
    /// The connector lacks a property HDR needs.
    #[error("the connector has no {0} property")]
    Missing(&'static str),
    /// The connector's `Colorspace` has no BT.2020 RGB.
    #[error("the connector cannot signal BT.2020 RGB")]
    NoBt2020,
    /// The kernel refused.
    #[error("the kernel refused: {0}")]
    Refused(#[from] std::io::Error),
}

/// Puts the monitor on `connector` of `device` into HDR10, reaching
/// `peak` cd/m² as `capability` describes it, or out of it with none.
///
/// # Errors
///
/// Returns why not: the connector cannot, or the kernel refused (a frame
/// on its way, say: asking again later can work).
pub(crate) fn signal(
    device: &impl ControlDevice,
    connector: connector::Handle,
    hdr: Option<(&HdrCapability, f64)>,
) -> Result<(), HdrError> {
    let properties = device.get_properties(connector)?;
    let mut colorspace = None;
    let mut metadata = None;
    for (&handle, _) in properties.iter() {
        let info = device.get_property(handle)?;
        match info.name().to_bytes() {
            b"Colorspace" => colorspace = Some((handle, info)),
            b"HDR_OUTPUT_METADATA" => metadata = Some(handle),
            _ => {}
        }
    }
    let (colorspace, colorspace_info) = colorspace.ok_or(HdrError::Missing("Colorspace"))?;
    let metadata = metadata.ok_or(HdrError::Missing("HDR_OUTPUT_METADATA"))?;
    let property::ValueType::Enum(values) = colorspace_info.value_type() else {
        return Err(HdrError::Missing("Colorspace"));
    };
    let wanted_name: &[u8] = if hdr.is_some() {
        b"BT2020_RGB"
    } else {
        b"Default"
    };
    let (raw_values, enums) = values.values();
    let space = raw_values
        .iter()
        .zip(enums)
        .find(|(_, value)| value.name().to_bytes() == wanted_name)
        .map(|(raw, _)| *raw)
        .ok_or(HdrError::NoBt2020)?;
    let blob = match hdr {
        Some((capability, peak)) => {
            let blob = device.create_property_blob(&capability.metadata(peak))?;
            if let property::Value::Blob(id) = blob {
                id
            } else {
                0
            }
        }
        None => 0,
    };
    let mut request = AtomicModeReq::new();
    request.add_raw_property(connector.into(), colorspace, space);
    request.add_raw_property(connector.into(), metadata, blob);
    let committed = device.atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request);
    if blob != 0 {
        // The connector holds its own reference now, or never took one.
        let _ = device.destroy_property_blob(blob);
    }
    committed?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the little-endian u16 at byte `at` of `bytes`.
    fn word(bytes: &[u8; 32], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    #[test]
    fn the_metadata_is_the_kernels_layout() {
        let capability = HdrCapability {
            peak: 496.7,
            frame_average: 300.0,
            black: 0.05,
        };
        let bytes = capability.metadata(496.7);
        assert_eq!(&bytes[..4], &[0; 4], "metadata type 0");
        assert_eq!(bytes[4], 2, "PQ");
        assert_eq!(bytes[5], 0, "static metadata type 1's own type 0");
        assert_eq!((word(&bytes, 6), word(&bytes, 8)), (35400, 14600), "red");
        assert_eq!(
            (word(&bytes, 18), word(&bytes, 20)),
            (15635, 16450),
            "white"
        );
        assert_eq!(word(&bytes, 22), 497, "peak, cd/m²");
        assert_eq!(word(&bytes, 24), 500, "black, 0.0001 cd/m²");
        assert_eq!(word(&bytes, 26), 497, "max CLL");
        assert_eq!(word(&bytes, 28), 300, "max FALL");
        assert_eq!(&bytes[30..], &[0; 2], "padding");
    }
}
