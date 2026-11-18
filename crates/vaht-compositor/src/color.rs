//! Colour: what a surface's or an output's pixel values mean, and how
//! one's are turned into another's (HDR, `wp_color_management_v1`).
//!
//! An [`ImageDescription`] says which colours the primaries are, how
//! values are encoded (the transfer function) and how bright its white and
//! peak are. [`Conversion`] turns one into another, in absolute luminance:
//!
//! 1. the source's transfer function is undone, to linear light;
//! 2. the source's reference white is scaled to the target's, so an SDR
//!    window is as bright on an HDR monitor as the user chose;
//! 3. the gamut is changed (BT.709 to BT.2020, say), adapting the white
//!    point (Bradford) when the two differ;
//! 4. light above the target's peak is rolled off to it (tone mapping),
//!    keeping each colour's hue;
//! 5. the target's transfer function encodes it.
//!
//! The same steps run in the shaders that draw surfaces
//! ([`crate::scene`]'s colour-converted elements) and here, on the CPU,
//! for solid colours and the tests. Smithay has no colour management yet
//! (Smithay#982 and #1143 are drafts); `tools/smithay-watch` says when it
//! does.

/// A colour's chromaticity, CIE 1931 `x` and `y`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Chromaticity {
    /// `x`.
    pub(crate) x: f64,
    /// `y`.
    pub(crate) y: f64,
}

impl Chromaticity {
    /// Returns the chromaticity `(x, y)`.
    pub(crate) const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Returns its XYZ, at luminance `Y` 1.
    fn xyz(self) -> [f64; 3] {
        [self.x / self.y, 1.0, (1.0 - self.x - self.y) / self.y]
    }
}

/// The chromaticities of the red, green and blue primaries and of white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Primaries {
    /// Red.
    pub(crate) red: Chromaticity,
    /// Green.
    pub(crate) green: Chromaticity,
    /// Blue.
    pub(crate) blue: Chromaticity,
    /// White.
    pub(crate) white: Chromaticity,
}

/// The white of daylight, D65: sRGB's, BT.2020's and Display P3's.
const D65: Chromaticity = Chromaticity::new(0.3127, 0.3290);

impl Primaries {
    /// sRGB's and BT.709's.
    pub(crate) const SRGB: Self = Self {
        red: Chromaticity::new(0.64, 0.33),
        green: Chromaticity::new(0.30, 0.60),
        blue: Chromaticity::new(0.15, 0.06),
        white: D65,
    };

    /// BT.2020's and BT.2100's: HDR's.
    pub(crate) const BT2020: Self = Self {
        red: Chromaticity::new(0.708, 0.292),
        green: Chromaticity::new(0.170, 0.797),
        blue: Chromaticity::new(0.131, 0.046),
        white: D65,
    };

    /// Display P3's.
    pub(crate) const DISPLAY_P3: Self = Self {
        red: Chromaticity::new(0.680, 0.320),
        green: Chromaticity::new(0.265, 0.690),
        blue: Chromaticity::new(0.150, 0.060),
        white: D65,
    };

    /// Returns the matrix turning linear RGB in these primaries into XYZ.
    fn to_xyz(self) -> Matrix {
        let columns = [self.red.xyz(), self.green.xyz(), self.blue.xyz()];
        let unscaled = Matrix::from_columns(columns);
        // Scaled so RGB 1, 1, 1 is the white point at Y 1.
        let scale = unscaled.inverse().apply(self.white.xyz());
        Matrix::from_columns([
            columns[0].map(|value| value * scale[0]),
            columns[1].map(|value| value * scale[1]),
            columns[2].map(|value| value * scale[2]),
        ])
    }
}

/// How pixel values encode light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TransferFunction {
    /// sRGB's piecewise curve.
    Srgb,
    /// A pure power curve, `value ^ exponent` (gamma 2.2, BT.1886's 2.4).
    Power(f64),
    /// Linear light.
    Linear,
    /// SMPTE ST 2084, perceptual quantizer: HDR10's, in absolute luminance
    /// up to 10 000 cd/m².
    Pq,
}

impl TransferFunction {
    /// The brightest light PQ encodes, in cd/m².
    pub(crate) const PQ_PEAK: f64 = 10_000.0;

    /// Returns whether values are absolute luminance, not relative to the
    /// description's white.
    pub(crate) const fn is_absolute(self) -> bool {
        matches!(self, Self::Pq)
    }

    /// Returns the linear light `encoded` stands for: relative (1 is the
    /// description's peak), or for PQ a share of 10 000 cd/m².
    pub(crate) fn decode(self, encoded: f64) -> f64 {
        match self {
            Self::Srgb => {
                let magnitude = encoded.abs();
                let linear = if magnitude <= 0.040_45 {
                    magnitude / 12.92
                } else {
                    ((magnitude + 0.055) / 1.055).powf(2.4)
                };
                linear.copysign(encoded)
            }
            Self::Power(exponent) => encoded.max(0.0).powf(exponent),
            Self::Linear => encoded,
            Self::Pq => {
                let power = encoded.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
                ((power - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * power)).powf(1.0 / PQ_M1)
            }
        }
    }

    /// Returns the encoding of linear light `linear`, as [`Self::decode`]
    /// takes it.
    pub(crate) fn encode(self, linear: f64) -> f64 {
        match self {
            Self::Srgb => {
                let magnitude = linear.abs();
                let encoded = if magnitude <= 0.003_130_8 {
                    magnitude * 12.92
                } else {
                    1.055 * magnitude.powf(1.0 / 2.4) - 0.055
                };
                encoded.copysign(linear)
            }
            Self::Power(exponent) => linear.max(0.0).powf(1.0 / exponent),
            Self::Linear => linear,
            Self::Pq => {
                let power = linear.clamp(0.0, 1.0).powf(PQ_M1);
                ((PQ_C1 + PQ_C2 * power) / (1.0 + PQ_C3 * power)).powf(PQ_M2)
            }
        }
    }
}

/// PQ's constants (SMPTE ST 2084).
const PQ_M1: f64 = 2610.0 / 16384.0;
/// See [`PQ_M1`].
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
/// See [`PQ_M1`].
const PQ_C1: f64 = 3424.0 / 4096.0;
/// See [`PQ_M1`].
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
/// See [`PQ_M1`].
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// How bright a description's black, peak and reference white are, in
/// cd/m².
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Luminances {
    /// Black.
    pub(crate) min: f64,
    /// The peak: for relative transfer functions, what encoded 1 shows.
    pub(crate) max: f64,
    /// Reference white: a document's white, an SDR window's.
    pub(crate) reference: f64,
}

impl Luminances {
    /// The protocol's default for SDR transfer functions: an sRGB display.
    pub(crate) const SDR: Self = Self {
        min: 0.2,
        max: 80.0,
        reference: 80.0,
    };

    /// The protocol's default for PQ: BT.2100's reference white, 203.
    pub(crate) const PQ: Self = Self {
        min: 0.005,
        max: TransferFunction::PQ_PEAK,
        reference: 203.0,
    };
}

/// What a surface's or output's pixel values mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ImageDescription {
    /// The colours of its primaries and white.
    pub(crate) primaries: Primaries,
    /// How values encode light.
    pub(crate) transfer: TransferFunction,
    /// How bright black, the peak and reference white are.
    pub(crate) luminances: Luminances,
    /// The brightest light of its target colour volume, when known, in
    /// cd/m²: content's mastering display's peak (or its max CLL), a
    /// monitor's own peak. Tone mapping works from it rather than the
    /// encoding's range (PQ's 10 000).
    pub(crate) target_peak: Option<f64>,
}

impl ImageDescription {
    /// sRGB: what every surface is unless it says otherwise.
    pub(crate) const SRGB: Self = Self {
        primaries: Primaries::SRGB,
        transfer: TransferFunction::Srgb,
        luminances: Luminances::SDR,
        target_peak: None,
    };

    /// HDR10 (BT.2100 PQ) on a monitor reaching `peak` cd/m², with SDR
    /// white at `reference`.
    pub(crate) const fn hdr10(peak: f64, reference: f64) -> Self {
        Self {
            primaries: Primaries::BT2020,
            transfer: TransferFunction::Pq,
            luminances: Luminances {
                reference,
                ..Luminances::PQ
            },
            target_peak: Some(peak),
        }
    }

    /// Returns the luminance, in cd/m², of linear value `linear` (as
    /// [`TransferFunction::decode`] gives it).
    fn nits(&self, linear: f64) -> f64 {
        if self.transfer.is_absolute() {
            linear * TransferFunction::PQ_PEAK
        } else {
            linear * self.luminances.max
        }
    }

    /// Returns the linear value of luminance `nits`, as
    /// [`TransferFunction::encode`] takes it.
    fn linear(&self, nits: f64) -> f64 {
        if self.transfer.is_absolute() {
            nits / TransferFunction::PQ_PEAK
        } else {
            nits / self.luminances.max
        }
    }

    /// Returns the brightest light this description's content reaches, in
    /// cd/m².
    fn peak(&self) -> f64 {
        self.target_peak.unwrap_or(self.luminances.max)
    }
}

impl ImageDescription {
    /// Returns a number standing for the description: the same for equal
    /// descriptions, and in practice different for different ones; never 0
    /// (the protocol's `identity`).
    pub(crate) fn identity(&self) -> u32 {
        let mut hasher = std::hash::DefaultHasher::new();
        for number in self.numbers() {
            std::hash::Hash::hash(&number.to_bits(), &mut hasher);
        }
        let hash = std::hash::Hasher::finish(&hasher);
        let folded = u32::try_from(hash & u64::from(u32::MAX)).unwrap_or(1)
            ^ u32::try_from(hash >> 32).unwrap_or(0);
        folded.max(1)
    }

    /// Returns every number the description is made of, for
    /// [`Self::identity`].
    fn numbers(&self) -> Vec<f64> {
        let chromaticities = [
            self.primaries.red,
            self.primaries.green,
            self.primaries.blue,
            self.primaries.white,
        ];
        let transfer = match self.transfer {
            TransferFunction::Srgb => [1.0, 0.0],
            TransferFunction::Power(exponent) => [2.0, exponent],
            TransferFunction::Linear => [3.0, 0.0],
            TransferFunction::Pq => [4.0, 0.0],
        };
        chromaticities
            .iter()
            .flat_map(|chromaticity| [chromaticity.x, chromaticity.y])
            .chain(transfer)
            .chain([
                self.luminances.min,
                self.luminances.max,
                self.luminances.reference,
                self.target_peak.unwrap_or(-1.0),
            ])
            .collect()
    }

    /// Returns what `output` shows: HDR10 while it is in HDR, else sRGB.
    pub(crate) fn of_output(output: &smithay::output::Output) -> Self {
        output
            .user_data()
            .get::<OutputColor>()
            .and_then(|color| color.0.lock().ok().map(|description| *description))
            .unwrap_or(Self::SRGB)
    }

    /// Records that `output` now shows this; returns whether that changed
    /// it.
    pub(crate) fn set_for_output(self, output: &smithay::output::Output) -> bool {
        let color = output.user_data().get_or_insert_threadsafe(|| {
            OutputColor(std::sync::Mutex::new(ImageDescription::SRGB))
        });
        let Ok(mut current) = color.0.lock() else {
            return false;
        };
        let has_changed = *current != self;
        *current = self;
        has_changed
    }
}

/// What an output shows, kept with it.
#[derive(Debug)]
struct OutputColor(std::sync::Mutex<ImageDescription>);

/// How one description's pixels are turned into another's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Conversion {
    /// The source.
    pub(crate) source: ImageDescription,
    /// The target.
    pub(crate) target: ImageDescription,
    /// Turns the source's linear RGB, in cd/m² after scaling reference
    /// white, into the target's.
    pub(crate) gamut: Matrix,
    /// How much brighter the target's reference white is than the
    /// source's.
    pub(crate) white_scale: f64,
    /// The target's peak, in cd/m²: brighter light is rolled off to it.
    pub(crate) target_peak: f64,
    /// Where the roll-off starts, in cd/m²: below it light is kept as it
    /// is.
    pub(crate) knee: f64,
    /// The brightest light the source brings, after scaling, in cd/m².
    pub(crate) source_peak: f64,
}

impl Conversion {
    /// Where tone mapping starts, as a share of the target's peak.
    const KNEE: f64 = 0.75;

    /// Returns how `source`'s pixels become `target`'s.
    pub(crate) fn new(source: ImageDescription, target: ImageDescription) -> Self {
        let adaptation = bradford(source.primaries.white, target.primaries.white);
        let gamut = target
            .primaries
            .to_xyz()
            .inverse()
            .times(&adaptation)
            .times(&source.primaries.to_xyz());
        let white_scale = target.luminances.reference / source.luminances.reference;
        let target_peak = target.peak();
        Self {
            source,
            target,
            gamut,
            white_scale,
            target_peak,
            knee: target_peak * Self::KNEE,
            source_peak: source.peak() * white_scale,
        }
    }

    /// Returns the luminance, in cd/m², of the source's linear 1, after
    /// scaling its reference white to the target's.
    pub(crate) fn source_scale(&self) -> f64 {
        self.source.nits(1.0) * self.white_scale
    }

    /// Returns the target's linear value of 1 cd/m².
    pub(crate) fn target_scale(&self) -> f64 {
        self.target.linear(1.0)
    }

    /// Returns whether it changes nothing: the target is the source.
    pub(crate) fn is_identity(&self) -> bool {
        self.source == self.target
    }

    /// Returns whether light needs rolling off: the source can be brighter
    /// than the target shows.
    pub(crate) fn tone_maps(&self) -> bool {
        self.source_peak > self.target_peak * (1.0 + 1e-6)
    }

    /// Returns `rgb`, encoded as the source says, encoded as the target
    /// says.
    pub(crate) fn apply(&self, rgb: [f64; 3]) -> [f64; 3] {
        if self.is_identity() {
            return rgb;
        }
        let nits = rgb
            .map(|value| self.source.nits(self.source.transfer.decode(value)) * self.white_scale);
        let converted = self.tone_map(self.gamut.apply(nits));
        converted.map(|value| {
            self.target
                .transfer
                .encode(self.target.linear(value).max(0.0))
        })
    }

    /// Rolls light above the knee off towards the target's peak, scaling
    /// each colour as a whole by its brightest channel, so its hue stays.
    fn tone_map(&self, nits: [f64; 3]) -> [f64; 3] {
        let brightest = nits[0].max(nits[1]).max(nits[2]);
        if !self.tone_maps() || brightest <= self.knee {
            return nits;
        }
        let mapped = roll_off(brightest, self.knee, self.source_peak, self.target_peak);
        nits.map(|value| value * mapped / brightest)
    }
}

/// Maps `nits`, above `knee`, from up to `source_peak` into up to
/// `target_peak`: an extended Reinhard curve, smooth at the knee and
/// reaching the target's peak at the source's.
pub(crate) fn roll_off(nits: f64, knee: f64, source_peak: f64, target_peak: f64) -> f64 {
    let room = target_peak - knee;
    let over = (nits - knee) / room;
    let most = (source_peak - knee) / room;
    let mapped = over * (1.0 + over / (most * most)) / (1.0 + over);
    knee + room * mapped.min(1.0)
}

/// Returns the Bradford matrix adapting XYZ colours under white `from` to
/// white `to`: the identity when they are the same.
fn bradford(from: Chromaticity, to: Chromaticity) -> Matrix {
    if from == to {
        return Matrix::IDENTITY;
    }
    let cone = Matrix {
        rows: [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ],
    };
    let (source, target) = (cone.apply(from.xyz()), cone.apply(to.xyz()));
    let scale = Matrix {
        rows: [
            [target[0] / source[0], 0.0, 0.0],
            [0.0, target[1] / source[1], 0.0],
            [0.0, 0.0, target[2] / source[2]],
        ],
    };
    cone.inverse().times(&scale).times(&cone)
}

/// A 3 × 3 matrix, by rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Matrix {
    /// Its rows.
    pub(crate) rows: [[f64; 3]; 3],
}

impl Matrix {
    /// The identity.
    const IDENTITY: Self = Self {
        rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    /// Returns the matrix whose columns are `columns`.
    const fn from_columns(columns: [[f64; 3]; 3]) -> Self {
        let mut rows = [[0.0; 3]; 3];
        let mut row = 0;
        while row < 3 {
            let mut column = 0;
            while column < 3 {
                rows[row][column] = columns[column][row];
                column += 1;
            }
            row += 1;
        }
        Self { rows }
    }

    /// Returns `vector` multiplied by the matrix.
    pub(crate) fn apply(&self, vector: [f64; 3]) -> [f64; 3] {
        self.rows
            .map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
    }

    /// Returns this matrix times `other`.
    fn times(&self, other: &Self) -> Self {
        let mut rows = [[0.0; 3]; 3];
        for (row, out) in rows.iter_mut().enumerate() {
            for (column, cell) in out.iter_mut().enumerate() {
                *cell = (0..3)
                    .map(|index| self.rows[row][index] * other.rows[index][column])
                    .sum();
            }
        }
        Self { rows }
    }

    /// Returns the inverse; primaries' matrices always have one.
    fn inverse(&self) -> Self {
        // Each cell named by its row and column.
        let [[r0c0, r0c1, r0c2], [r1c0, r1c1, r1c2], [r2c0, r2c1, r2c2]] = self.rows;
        let cofactors = [
            [
                r1c1 * r2c2 - r1c2 * r2c1,
                r0c2 * r2c1 - r0c1 * r2c2,
                r0c1 * r1c2 - r0c2 * r1c1,
            ],
            [
                r1c2 * r2c0 - r1c0 * r2c2,
                r0c0 * r2c2 - r0c2 * r2c0,
                r0c2 * r1c0 - r0c0 * r1c2,
            ],
            [
                r1c0 * r2c1 - r1c1 * r2c0,
                r0c1 * r2c0 - r0c0 * r2c1,
                r0c0 * r1c1 - r0c1 * r1c0,
            ],
        ];
        let determinant = r0c0 * cofactors[0][0] + r0c1 * cofactors[1][0] + r0c2 * cofactors[2][0];
        Self {
            rows: cofactors.map(|row| row.map(|cell| cell / determinant)),
        }
    }

    /// Returns the matrix as the shaders take it: by columns, in `f32`.
    pub(crate) fn column_major(&self) -> [f32; 9] {
        let mut cells = [0.0; 9];
        for column in 0..3 {
            for row in 0..3 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a shader's precision is f32's"
                )]
                let cell = self.rows[row][column] as f32;
                cells[column * 3 + row] = cell;
            }
        }
        cells
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Asserts `actual` is within `tolerance` of `expected`.
    fn assert_near(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not {expected} (± {tolerance})"
        );
    }

    #[rstest]
    #[case::srgb(TransferFunction::Srgb)]
    #[case::gamma(TransferFunction::Power(2.2))]
    #[case::linear(TransferFunction::Linear)]
    #[case::pq(TransferFunction::Pq)]
    fn encoding_undoes_decoding(#[case] transfer: TransferFunction) {
        for step in 0..=20 {
            let encoded = f64::from(step) / 20.0;
            // PQ encodes no light as c1 ^ m2, 7.3e-7: a thousandth of a
            // 10-bit step.
            assert_near(transfer.encode(transfer.decode(encoded)), encoded, 1e-6);
        }
    }

    #[rstest]
    // BT.2100's table: reference white, 203 cd/m², is 58 % of PQ's range.
    #[case::reference_white(203.0, 0.5807)]
    #[case::hundred(100.0, 0.5081)]
    #[case::peak(10_000.0, 1.0)]
    #[case::black(0.0, 0.0)]
    fn pq_encodes_absolute_luminance(#[case] nits: f64, #[case] encoded: f64) {
        let value = TransferFunction::Pq.encode(nits / TransferFunction::PQ_PEAK);
        assert_near(value, encoded, 5e-4);
    }

    #[test]
    fn srgb_half_grey_is_a_fifth_of_white() {
        assert_near(TransferFunction::Srgb.decode(0.5), 0.2140, 1e-4);
    }

    #[test]
    fn bt709_to_bt2020_is_bt2087s_matrix() {
        let gamut = Conversion::new(
            ImageDescription::SRGB,
            ImageDescription {
                primaries: Primaries::BT2020,
                ..ImageDescription::SRGB
            },
        )
        .gamut;
        let expected = [
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ];
        for (row, expected_row) in gamut.rows.iter().zip(expected) {
            for (cell, expected_cell) in row.iter().zip(expected_row) {
                assert_near(*cell, expected_cell, 2e-4);
            }
        }
    }

    #[test]
    fn the_same_description_converts_to_itself() {
        let conversion = Conversion::new(ImageDescription::SRGB, ImageDescription::SRGB);
        assert!(conversion.is_identity());
        assert_eq!(conversion.apply([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]);
    }

    #[test]
    fn sdr_white_on_an_hdr_monitor_is_the_chosen_white() {
        let output = ImageDescription::hdr10(500.0, 203.0);
        let white = Conversion::new(ImageDescription::SRGB, output).apply([1.0; 3]);
        let expected = TransferFunction::Pq.encode(203.0 / TransferFunction::PQ_PEAK);
        for channel in white {
            assert_near(channel, expected, 1e-6);
        }
    }

    #[test]
    fn sdr_black_stays_black_on_an_hdr_monitor() {
        let output = ImageDescription::hdr10(500.0, 203.0);
        let black = Conversion::new(ImageDescription::SRGB, output).apply([0.0; 3]);
        for channel in black {
            assert_near(channel, 0.0, 1e-6);
        }
    }

    #[test]
    fn hdr_brighter_than_the_monitor_is_rolled_off_to_its_peak() {
        let content = ImageDescription::hdr10(10_000.0, 203.0);
        let output = ImageDescription::hdr10(500.0, 203.0);
        let conversion = Conversion::new(content, output);
        let peak = conversion.apply([1.0; 3]);
        let shown = TransferFunction::Pq.decode(peak[0]) * TransferFunction::PQ_PEAK;
        assert_near(shown, 500.0, 0.5);
        // Below the knee, light is kept.
        let dim = TransferFunction::Pq.encode(100.0 / TransferFunction::PQ_PEAK);
        assert_near(conversion.apply([dim; 3])[0], dim, 1e-9);
    }

    #[test]
    fn rolling_off_keeps_hue() {
        let content = ImageDescription::hdr10(10_000.0, 203.0);
        let output = ImageDescription::hdr10(500.0, 203.0);
        let conversion = Conversion::new(content, output);
        let encoded = [0.9, 0.6, 0.3];
        let nits = |rgb: [f64; 3]| {
            rgb.map(|value| TransferFunction::Pq.decode(value) * TransferFunction::PQ_PEAK)
        };
        let (before, after) = (nits(encoded), nits(conversion.apply(encoded)));
        assert_near(after[1] / after[0], before[1] / before[0], 1e-6);
        assert_near(after[2] / after[0], before[2] / before[0], 1e-6);
    }

    #[test]
    fn rolling_off_is_continuous_at_the_knee_and_reaches_the_peak() {
        assert_near(roll_off(375.0, 375.0, 10_000.0, 500.0), 375.0, 1e-9);
        assert_near(roll_off(10_000.0, 375.0, 10_000.0, 500.0), 500.0, 1e-9);
        assert!(roll_off(1000.0, 375.0, 10_000.0, 500.0) < 500.0);
    }

    #[test]
    fn hdr_on_an_sdr_monitor_puts_reference_white_at_its_white() {
        let content = ImageDescription::hdr10(1000.0, 203.0);
        let conversion = Conversion::new(content, ImageDescription::SRGB);
        let white = TransferFunction::Pq.encode(203.0 / TransferFunction::PQ_PEAK);
        let shown = conversion.apply([white; 3]);
        // Reference white is 80 cd/m² on sRGB, its peak: rolled off a
        // little below 1.
        assert!(shown[0] > 0.9 && shown[0] <= 1.0, "{shown:?}");
    }
}
