//! Describing the configuration to programs that edit it, as JSON Schema
//! (draft 2020-12).
//!
//! The settings types derive their schemas from their own definitions, so
//! names, documentation, defaults and enum values cannot drift from what vaht
//! reads. A type that reads more than its fields say (a number within a
//! range, a string or a table) implements [`schemars::JsonSchema`] by hand
//! with the helpers here, taking every bound from the constants its own
//! validation uses.

use schemars::{Schema, json_schema};
use serde_json::Value;
use vaht_ipc::ConfigFile;

#[cfg(test)]
use super::Config;
use super::Settings;

/// Returns the schema of `file`, or none if it is not described yet.
///
/// # Panics
///
/// Never: a schema is plain data, always a JSON value.
pub(crate) fn schema_of(file: ConfigFile) -> Option<Value> {
    match file {
        ConfigFile::Settings => Some(
            serde_json::to_value(schemars::schema_for!(Settings))
                .expect("a schema is plain data, always a JSON value"),
        ),
        ConfigFile::Outputs | ConfigFile::Bindings | ConfigFile::Rules | ConfigFile::Menus => None,
    }
}

/// Returns `settings` as JSON, every key present; a key with no value (an
/// optional setting left out) is `null`.
///
/// # Panics
///
/// Never: settings are plain data, always a JSON value.
pub(crate) fn settings_value(settings: &Settings) -> Value {
    serde_json::to_value(settings).expect("settings are plain data, always a JSON value")
}

/// Returns the schema of a whole number from `minimum` to `maximum`.
pub(crate) fn bounded_integer(minimum: i64, maximum: i64) -> Schema {
    json_schema!({
        "type": "integer",
        "minimum": minimum,
        "maximum": maximum,
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Returns the schema of `settings.toml`, with `$ref`s left as they are.
    fn settings_schema() -> Value {
        schema_of(ConfigFile::Settings).expect("settings are described")
    }

    /// Returns the schema `reference` (`#/$defs/Name`) names in `root`.
    fn resolve<'schema>(root: &'schema Value, reference: &str) -> &'schema Value {
        let path = reference.strip_prefix('#').expect("a local reference");
        root.pointer(path).expect("the reference points somewhere")
    }

    /// Returns what an optional setting's schema (`anyOf` the type or
    /// `null`, "not set") says the type is.
    fn present_alternative(schema: &Value) -> Option<&Value> {
        let members = schema.get("anyOf")?.as_array()?;
        let is_null = |member: &&Value| member.get("type") == Some(&Value::from("null"));
        let mut present = members.iter().filter(|member| !is_null(member));
        let only = present.next()?;
        (present.next().is_none() && members.iter().any(|member| is_null(&member))).then_some(only)
    }

    /// Follows `schema`'s `$ref`s and single-member `allOf`s to the schema
    /// that says what it is.
    fn resolved<'schema>(root: &'schema Value, mut schema: &'schema Value) -> &'schema Value {
        loop {
            if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
                schema = resolve(root, reference);
            } else if let Some([only]) = schema
                .get("allOf")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
            {
                schema = only;
            } else if let Some(present) = present_alternative(schema) {
                schema = present;
            } else {
                return schema;
            }
        }
    }

    /// Returns the schema of the setting at `path`, if the schema describes
    /// it: through `properties`, and through `additionalProperties` for
    /// tables whose keys are free (a device's name).
    fn setting_schema<'schema>(root: &'schema Value, path: &[&str]) -> Option<&'schema Value> {
        let mut node = root;
        for part in path {
            let current = resolved(root, node);
            node = current
                .pointer(&format!("/properties/{part}"))
                .or_else(|| current.get("additionalProperties"))?;
        }
        Some(resolved(root, node))
    }

    /// Collects the path of every key `table` holds, tables followed in.
    fn key_paths(table: &toml::Table, prefix: &[String], paths: &mut Vec<Vec<String>>) {
        for (key, value) in table {
            let mut path = prefix.to_vec();
            path.push(key.clone());
            paths.push(path.clone());
            if let toml::Value::Table(inner) = value {
                key_paths(inner, &path, paths);
            }
        }
    }

    #[test]
    fn every_key_of_the_default_settings_file_is_described() {
        let defaults: toml::Table = Config::FILES
            .iter()
            .find(|file| file.name == "settings.toml")
            .expect("settings.toml is a configuration file")
            .defaults
            .parse()
            .expect("the defaults are TOML");
        let schema = settings_schema();
        let mut paths = Vec::new();
        key_paths(&defaults, &[], &mut paths);
        assert_ne!(paths.len(), 0, "the defaults file has settings");
        for path in paths {
            let parts: Vec<&str> = path.iter().map(String::as_str).collect();
            assert!(
                setting_schema(&schema, &parts).is_some(),
                "{} is not in the schema",
                path.join(".")
            );
        }
    }

    #[test]
    fn the_settings_survive_json_and_back() {
        let value = settings_value(&Settings::default());
        let table = json_to_toml(&value);
        let back = Settings::from_table(table).expect("the effective settings are settings");
        assert_eq!(back, Settings::default());
    }

    /// Converts a JSON object to a TOML table, leaving out `null`s: a key
    /// with no value is a key the file does not set.
    fn json_to_toml(value: &Value) -> toml::Table {
        fn convert(value: &Value) -> Option<toml::Value> {
            match value {
                Value::Null => None,
                Value::Bool(flag) => Some(toml::Value::Boolean(*flag)),
                Value::Number(number) => number
                    .as_i64()
                    .map(toml::Value::Integer)
                    .or_else(|| number.as_f64().map(toml::Value::Float)),
                Value::String(text) => Some(toml::Value::String(text.clone())),
                Value::Array(items) => Some(toml::Value::Array(
                    items.iter().filter_map(convert).collect(),
                )),
                Value::Object(map) => Some(toml::Value::Table(
                    map.iter()
                        .filter_map(|(key, inner)| Some((key.clone(), convert(inner)?)))
                        .collect(),
                )),
            }
        }
        match convert(value) {
            Some(toml::Value::Table(table)) => table,
            Some(_) | None => panic!("settings are an object"),
        }
    }

    /// Returns whether `settings.toml` setting `path` = `value` is accepted.
    fn is_accepted(path: &[&str], value: toml::Value) -> bool {
        let mut inner = value;
        for part in path.iter().rev() {
            let mut table = toml::Table::new();
            table.insert((*part).to_owned(), inner);
            inner = toml::Value::Table(table);
        }
        let toml::Value::Table(table) = inner else {
            panic!("a path has a part");
        };
        Settings::from_table(table).is_ok()
    }

    #[rstest]
    #[case::blur_passes(&["blur", "passes"])]
    #[case::blur_spread(&["blur", "spread"])]
    #[case::log_file_size(&["logging", "file_size_mb"])]
    #[case::log_files(&["logging", "files"])]
    #[case::log_days(&["logging", "delete_after_days"])]
    #[case::taskbar_width(&["taskbar", "width_percent"])]
    #[case::pointer_speed(&["input", "all", "accel_speed"])]
    fn a_range_in_the_schema_is_what_vaht_accepts(#[case] path: &[&str]) {
        let schema = settings_schema();
        let setting = setting_schema(&schema, path).expect("the setting is described");
        let bound = |keyword: &str| {
            setting
                .get(keyword)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("{path:?} has no {keyword}: {setting}"))
        };
        let (minimum, maximum) = (bound("minimum"), bound("maximum"));
        let value = |number: f64| {
            if setting.get("type") == Some(&Value::from("integer")) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "whole bounds, far from i64's"
                )]
                toml::Value::Integer(number as i64)
            } else {
                toml::Value::Float(number)
            }
        };
        let step = if setting.get("type") == Some(&Value::from("integer")) {
            1.0
        } else {
            0.01
        };
        assert!(is_accepted(path, value(minimum)), "the minimum is accepted");
        assert!(is_accepted(path, value(maximum)), "the maximum is accepted");
        assert!(
            !is_accepted(path, value(minimum - step)),
            "below the minimum is refused"
        );
        assert!(
            !is_accepted(path, value(maximum + step)),
            "above the maximum is refused"
        );
    }

    #[test]
    fn a_setting_is_described_with_its_documentation_and_default() {
        let schema = settings_schema();
        let model = setting_schema(&schema, &["focus", "model"]).expect("described");
        assert!(
            model.get("enum").or_else(|| model.get("oneOf")).is_some(),
            "{model}"
        );
        let focus = resolved(&schema, schema.pointer("/properties/focus").expect("focus"));
        let model_entry = focus.pointer("/properties/model").expect("the property");
        assert!(
            model_entry.get("description").is_some()
                || resolved(&schema, model_entry).get("description").is_some(),
            "documentation comes from the doc comment: {model_entry}"
        );
        assert!(
            focus.pointer("/properties/model/default").is_some()
                || model_entry.get("default").is_some()
                || focus.get("default").is_some(),
            "a default is said: {focus}"
        );
    }
}
