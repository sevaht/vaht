//! `[blur]`: blurring what is behind the parts of windows and panels that
//! ask for it (`ext-background-effect-v1`), as frosted glass.
//!
//! The blur is a dual Kawase blur: the background is halved in size
//! `passes` times and grown back as many, each step averaging neighbours
//! `spread` pixels apart, so more passes blur wider and a larger spread
//! blurs softer.

/// Whether and how strongly backgrounds are blurred.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct BlurSettings {
    /// Whether programs may have what is behind them blurred.
    pub(crate) enabled: bool,
    /// How many times the background is halved and grown back.
    pub(crate) passes: BlurPasses,
    /// How far apart, in pixels, each step's samples are.
    pub(crate) spread: BlurSpread,
}

impl Default for BlurSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            passes: BlurPasses(3),
            spread: BlurSpread(3),
        }
    }
}

/// How many times a blurred background is halved: from 1 to 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "u8")]
pub(crate) struct BlurPasses(u8);

impl BlurPasses {
    /// The most passes: past six, a background is a few pixels wide.
    const MAX: u8 = 6;

    /// Returns the number of passes.
    pub(crate) const fn get(self) -> u8 {
        self.0
    }
}

impl schemars::JsonSchema for BlurPasses {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BlurPasses".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        crate::config::schema::bounded_integer(1, i64::from(Self::MAX))
    }
}

impl TryFrom<u8> for BlurPasses {
    type Error = String;

    fn try_from(passes: u8) -> Result<Self, String> {
        (1..=Self::MAX)
            .contains(&passes)
            .then_some(Self(passes))
            .ok_or_else(|| format!("blur passes are from 1 to {}, not {passes}", Self::MAX))
    }
}

/// How far apart a blur step's samples are, in pixels: from 1 to 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "u8")]
pub(crate) struct BlurSpread(u8);

impl BlurSpread {
    /// The widest spread: past ten, the samples miss what is between them
    /// and the blur shows bands.
    const MAX: u8 = 10;

    /// Returns the spread, in pixels.
    pub(crate) const fn get(self) -> u8 {
        self.0
    }
}

impl schemars::JsonSchema for BlurSpread {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BlurSpread".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        crate::config::schema::bounded_integer(1, i64::from(Self::MAX))
    }
}

impl TryFrom<u8> for BlurSpread {
    type Error = String;

    fn try_from(spread: u8) -> Result<Self, String> {
        (1..=Self::MAX)
            .contains(&spread)
            .then_some(Self(spread))
            .ok_or_else(|| format!("blur spread is from 1 to {}, not {spread}", Self::MAX))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Reads `text` as `[blur]`.
    fn blur(text: &str) -> Result<BlurSettings, String> {
        toml::from_str(text).map_err(|error: toml::de::Error| error.message().to_owned())
    }

    #[test]
    fn blur_is_on_by_default_with_three_passes() {
        let settings = blur("").expect("valid");
        assert!(settings.enabled);
        assert_eq!((settings.passes.get(), settings.spread.get()), (3, 3));
    }

    #[rstest]
    #[case::no_passes("passes = 0", "from 1 to 6")]
    #[case::too_many_passes("passes = 7", "from 1 to 6")]
    #[case::no_spread("spread = 0", "from 1 to 10")]
    #[case::unknown("radius = 4", "unknown field")]
    fn mistakes_are_explained(#[case] text: &str, #[case] expected: &str) {
        let error = blur(text).expect_err(text);
        assert!(error.contains(expected), "{error}");
    }
}
