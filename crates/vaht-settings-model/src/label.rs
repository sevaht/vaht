//! Words for a setting's key: what the window calls it.
//!
//! Keys are written `auto_raise_delay_ms`. The window says "Auto raise
//! delay" and shows "ms" as the unit beside the value. The suffixes are the
//! settings' own naming convention (`_ms`, `_mb`, `_days`, `_percent`).

/// A key's suffix that names a unit, and the unit it names.
const UNITS: [(&str, &str); 5] = [
    ("_ms", "ms"),
    ("_mb", "MB"),
    ("_days", "days"),
    ("_percent", "%"),
    ("_px", "px"),
];

/// Returns the words of `key` for a person to read, and the unit its name
/// ends in, if it ends in one: `auto_raise_delay_ms` is "Auto raise delay"
/// in "ms".
pub(crate) fn of_key(key: &str) -> (String, Option<&'static str>) {
    let (stem, unit) = UNITS
        .iter()
        .find_map(|(suffix, unit)| key.strip_suffix(suffix).map(|stem| (stem, Some(*unit))))
        .unwrap_or((key, None));
    let mut words = stem.replace(['_', '-'], " ");
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    (words, unit)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::plain("model", "Model", None)]
    #[case::words("raise_on_click", "Raise on click", None)]
    #[case::milliseconds("auto_raise_delay_ms", "Auto raise delay", Some("ms"))]
    #[case::megabytes("file_size_mb", "File size", Some("MB"))]
    #[case::percent("width_percent", "Width", Some("%"))]
    #[case::kebab("some-key", "Some key", None)]
    fn a_key_becomes_words_and_a_unit(
        #[case] key: &str,
        #[case] words: &str,
        #[case] unit: Option<&str>,
    ) {
        assert_eq!(of_key(key), (words.to_owned(), unit));
    }
}
