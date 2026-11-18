//! The form: the settings as sections of values to fill in.
//!
//! [`Form::from_schema`] reads the JSON Schema vaht serves. A struct becomes
//! a [`Section`], and each leaf a [`Setting`] with the kind of value it takes
//! ([`SettingKind`]), the words to call it, its documentation, its default
//! and whether it may be unset. What the schema cannot say as a plain kind (a
//! number or the word "auto", a list of tables) is [`SettingKind::Raw`]: the
//! window offers its JSON until a purpose-built editor takes it over.

use serde_json::Value;
use vaht_ipc::ConfigPath;

use crate::label;

/// Why a schema is not a form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormError {
    /// The schema's root is not an object with properties.
    #[error("the schema describes no settings")]
    NoSettings,
    /// A `$ref` names a definition the schema lacks.
    #[error("the schema refers to `{0}`, which it does not define")]
    UnresolvedReference(String),
}

/// What kind of value a setting takes.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    /// On or off.
    Toggle,
    /// A whole number, perhaps within limits.
    Whole {
        /// The least it may be.
        minimum: Option<i64>,
        /// The most it may be.
        maximum: Option<i64>,
    },
    /// A number with a fraction, perhaps within limits.
    Decimal {
        /// The least it may be.
        minimum: Option<f64>,
        /// The most it may be.
        maximum: Option<f64>,
    },
    /// Free text.
    Text,
    /// One of a fixed set of words.
    Choice(Vec<Choice>),
    /// A list of values of one kind.
    List(Box<SettingKind>),
    /// Values of one kind by free-form name (a workspace's number, a device's
    /// name).
    Mapping(Box<SettingKind>),
    /// Anything else, edited as JSON.
    Raw,
}

/// One word a [`SettingKind::Choice`] allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The word, as the file writes it.
    pub value: String,
    /// What it means, from the setting's documentation; may be empty.
    pub description: String,
}

/// One setting: a value in the file, with its key, documentation and kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    /// Where it is in the file.
    pub path: ConfigPath,
    /// What the window calls it.
    pub label: String,
    /// The unit its value is in, if the key's name says one.
    pub unit: Option<&'static str>,
    /// What it does, from the documentation in vaht's code.
    pub description: String,
    /// What vaht uses when the file leaves it out; none if it is optional
    /// and has no default.
    pub default: Option<Value>,
    /// What kind of value it takes.
    pub kind: SettingKind,
    /// Whether it may be left unset: no value at all, not a default one.
    pub is_optional: bool,
}

/// A group of settings, a table in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Where it is in the file; empty for the top.
    pub path: Vec<String>,
    /// What the window calls it.
    pub label: String,
    /// What it is for.
    pub description: String,
    /// Its settings, in key order.
    pub settings: Vec<Setting>,
    /// Its tables, in key order.
    pub sections: Vec<Section>,
}

impl Section {
    /// Returns every setting in this section and those inside it, depth
    /// first.
    #[must_use]
    pub fn all_settings(&self) -> Vec<&Setting> {
        let mut all: Vec<&Setting> = self.settings.iter().collect();
        for section in &self.sections {
            all.extend(section.all_settings());
        }
        all
    }
}

/// The settings, as sections to fill in.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The top: its own settings, and a section for each table.
    root: Section,
}

impl Form {
    /// Builds the form `schema` (JSON Schema, as vaht serves it) describes.
    ///
    /// # Errors
    ///
    /// Returns why the schema is not a form.
    pub fn from_schema(schema: &Value) -> Result<Self, FormError> {
        let reader = SchemaReader { root: schema };
        let root = reader.section(&[], schema, schema)?;
        if root.settings.is_empty() && root.sections.is_empty() {
            return Err(FormError::NoSettings);
        }
        Ok(Self { root })
    }

    /// Returns the top of the form.
    #[must_use]
    pub const fn root(&self) -> &Section {
        &self.root
    }

    /// Returns every setting, in the order the sections list them.
    #[must_use]
    pub fn settings(&self) -> Vec<&Setting> {
        self.root.all_settings()
    }

    /// Returns the setting at `path`, if the form has one there.
    #[must_use]
    pub fn setting(&self, path: &ConfigPath) -> Option<&Setting> {
        self.settings()
            .into_iter()
            .find(|setting| &setting.path == path)
    }
}

/// Reads one schema document.
struct SchemaReader<'schema> {
    /// The whole schema, where `$ref`s point.
    root: &'schema Value,
}

/// What a schema node says once its references and optionality are
/// resolved.
struct Resolved<'schema> {
    /// The node that says what the type is.
    schema: &'schema Value,
    /// Whether it may be absent (`anyOf` the type or `null`).
    is_optional: bool,
}

impl<'schema> SchemaReader<'schema> {
    /// Reads `schema` (a struct's, at `path`) as a section; `outer` is the
    /// node that referred to it, whose documentation wins.
    fn section(
        &self,
        path: &[String],
        schema: &'schema Value,
        outer: &'schema Value,
    ) -> Result<Section, FormError> {
        let resolved = self.resolve(schema)?;
        let mut section = Section {
            path: path.to_vec(),
            label: path
                .last()
                .map_or_else(|| "Settings".to_owned(), |key| label::of_key(key).0),
            description: Self::description_of(outer, resolved.schema),
            settings: Vec::new(),
            sections: Vec::new(),
        };
        let Some(properties) = resolved.schema.get("properties").and_then(Value::as_object) else {
            return Ok(section);
        };
        for (key, property) in properties {
            let mut property_path = path.to_vec();
            property_path.push(key.clone());
            let resolved = self.resolve(property)?;
            if Self::is_struct(resolved.schema) && !resolved.is_optional {
                section
                    .sections
                    .push(self.section(&property_path, property, property)?);
            } else {
                section
                    .settings
                    .push(self.setting(property_path, property, &resolved));
            }
        }
        Ok(section)
    }

    /// Reads `property`, at `path`, as a setting.
    fn setting(
        &self,
        path: Vec<String>,
        property: &'schema Value,
        resolved: &Resolved<'_>,
    ) -> Setting {
        let (label, unit) = path
            .last()
            .map_or_else(|| (String::new(), None), |key| label::of_key(key));
        let default = property
            .get("default")
            .or_else(|| resolved.schema.get("default"))
            .filter(|default| !default.is_null())
            .cloned();
        Setting {
            path: ConfigPath::from_parts(path).expect("a setting has a key"),
            label,
            unit,
            description: Self::description_of(property, resolved.schema),
            default,
            kind: self.kind_of(resolved.schema),
            is_optional: resolved.is_optional,
        }
    }

    /// Returns what `outer` or, failing that, `inner` says it is for.
    fn description_of(outer: &Value, inner: &Value) -> String {
        outer
            .get("description")
            .or_else(|| inner.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// Returns whether `schema` is a struct: an object with named keys.
    fn is_struct(schema: &Value) -> bool {
        schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
    }

    /// Follows `$ref`s, a lone `allOf` and an optional's `anyOf` to the node
    /// that says what `schema` is.
    fn resolve(&self, schema: &'schema Value) -> Result<Resolved<'schema>, FormError> {
        let mut schema = schema;
        let mut is_optional = false;
        loop {
            if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
                let target = reference
                    .strip_prefix('#')
                    .and_then(|pointer| self.root.pointer(pointer))
                    .ok_or_else(|| FormError::UnresolvedReference(reference.to_owned()))?;
                schema = target;
            } else if let Some([only]) = schema
                .get("allOf")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
            {
                schema = only;
            } else if let Some(present) = Self::present_alternative(schema) {
                is_optional = true;
                schema = present;
            } else {
                return Ok(Resolved {
                    schema,
                    is_optional,
                });
            }
        }
    }

    /// Returns the type an optional setting's schema (`anyOf` it or `null`)
    /// says, if `schema` is one.
    fn present_alternative(schema: &Value) -> Option<&Value> {
        let members = schema.get("anyOf")?.as_array()?;
        let is_null = |member: &&Value| member.get("type") == Some(&Value::from("null"));
        let mut present = members.iter().filter(|member| !is_null(member));
        let only = present.next()?;
        (present.next().is_none() && members.iter().any(|member| is_null(&member))).then_some(only)
    }

    /// Returns the kind of value `schema` describes.
    fn kind_of(&self, schema: &'schema Value) -> SettingKind {
        if let Some(choices) = Self::choices_of(schema) {
            return SettingKind::Choice(choices);
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("boolean") => SettingKind::Toggle,
            Some("integer") => SettingKind::Whole {
                minimum: schema.get("minimum").and_then(Value::as_i64),
                maximum: schema.get("maximum").and_then(Value::as_i64),
            },
            Some("number") => SettingKind::Decimal {
                minimum: schema.get("minimum").and_then(Value::as_f64),
                maximum: schema.get("maximum").and_then(Value::as_f64),
            },
            Some("string") => SettingKind::Text,
            Some("array") => self.container(schema.get("items"), SettingKind::List),
            Some("object") if !Self::is_struct(schema) => self.container(
                schema
                    .get("additionalProperties")
                    .filter(|values| values.is_object()),
                SettingKind::Mapping,
            ),
            Some(_) | None => SettingKind::Raw,
        }
    }

    /// Returns `wrap` of the kind `items` describes, or raw if it is not a
    /// plain kind: a list or mapping of tables is for a dedicated editor.
    fn container(
        &self,
        items: Option<&'schema Value>,
        wrap: fn(Box<SettingKind>) -> SettingKind,
    ) -> SettingKind {
        let Some(items) = items else {
            return SettingKind::Raw;
        };
        let Ok(resolved) = self.resolve(items) else {
            return SettingKind::Raw;
        };
        let inner = self.kind_of(resolved.schema);
        if inner == SettingKind::Raw {
            SettingKind::Raw
        } else {
            wrap(Box::new(inner))
        }
    }

    /// Returns the words `schema` allows, if it is a choice among words: an
    /// `enum`, or a `oneOf` whose every member is a word `const`.
    fn choices_of(schema: &Value) -> Option<Vec<Choice>> {
        if let Some(words) = schema.get("enum").and_then(Value::as_array) {
            return words
                .iter()
                .map(|word| {
                    word.as_str().map(|value| Choice {
                        value: value.to_owned(),
                        description: String::new(),
                    })
                })
                .collect();
        }
        schema
            .get("oneOf")?
            .as_array()?
            .iter()
            .map(|member| {
                Some(Choice {
                    value: member.get("const")?.as_str()?.to_owned(),
                    description: member
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Returns the form of a schema with `$defs` and `properties`.
    fn form_of(defs: &Value, properties: &Value) -> Form {
        Form::from_schema(&json!({
            "$defs": defs,
            "type": "object",
            "properties": properties,
        }))
        .expect("a form")
    }

    /// Returns the setting at the dotted `key` of `form`.
    fn setting<'form>(form: &'form Form, key: &str) -> &'form Setting {
        form.setting(&ConfigPath::parse(key).expect("a key"))
            .unwrap_or_else(|| panic!("no setting {key}"))
    }

    #[test]
    fn a_struct_is_a_section_and_its_fields_are_settings() {
        let form = form_of(
            &json!({
                "FocusSettings": {
                    "type": "object",
                    "description": "How focus works.",
                    "properties": {
                        "auto_raise_delay_ms": {
                            "type": "integer", "minimum": 0, "default": 250,
                            "description": "How long the pointer rests."
                        },
                        "raise_on_click": { "type": "boolean", "default": true },
                    },
                },
            }),
            &json!({
                "focus": {
                    "$ref": "#/$defs/FocusSettings",
                    "description": "Which window gets focus.",
                },
            }),
        );
        let focus = &form.root().sections[0];
        assert_eq!(
            (focus.label.as_str(), focus.description.as_str()),
            ("Focus", "Which window gets focus."),
            "the referring node's documentation wins"
        );
        let delay = setting(&form, "focus.auto_raise_delay_ms");
        assert_eq!(delay.label, "Auto raise delay");
        assert_eq!(delay.unit, Some("ms"));
        assert_eq!(delay.default, Some(json!(250)));
        assert_eq!(delay.description, "How long the pointer rests.");
        assert_eq!(
            delay.kind,
            SettingKind::Whole {
                minimum: Some(0),
                maximum: None
            }
        );
        assert_eq!(
            setting(&form, "focus.raise_on_click").kind,
            SettingKind::Toggle
        );
    }

    #[test]
    fn a_choice_among_words_lists_them_with_their_meanings() {
        let form = form_of(
            &json!({
                "FocusModel": {
                    "oneOf": [
                        { "type": "string", "const": "click", "description": "Click to focus." },
                        { "type": "string", "const": "sloppy" },
                    ],
                },
            }),
            &json!({ "model": { "$ref": "#/$defs/FocusModel", "default": "click" } }),
        );
        let SettingKind::Choice(choices) = &setting(&form, "model").kind else {
            panic!("not a choice");
        };
        assert_eq!(
            choices,
            &[
                Choice {
                    value: "click".to_owned(),
                    description: "Click to focus.".to_owned()
                },
                Choice {
                    value: "sloppy".to_owned(),
                    description: String::new()
                },
            ]
        );
    }

    #[test]
    fn an_enum_of_words_is_a_choice_too() {
        let form = form_of(
            &json!({}),
            &json!({ "edge": { "enum": ["top", "bottom"] } }),
        );
        assert_eq!(
            setting(&form, "edge").kind,
            SettingKind::Choice(
                ["top", "bottom"]
                    .map(|value| Choice {
                        value: value.to_owned(),
                        description: String::new()
                    })
                    .to_vec()
            )
        );
    }

    #[test]
    fn an_optional_setting_is_the_type_it_names_and_may_be_unset() {
        let form = form_of(
            &json!({}),
            &json!({
                "days": {
                    "anyOf": [{ "type": "integer", "minimum": 1, "maximum": 3650 }, { "type": "null" }],
                    "default": null,
                },
            }),
        );
        let days = setting(&form, "days");
        assert!(days.is_optional);
        assert_eq!(
            days.default, None,
            "no default: unset is a state of its own"
        );
        assert_eq!(
            days.kind,
            SettingKind::Whole {
                minimum: Some(1),
                maximum: Some(3650)
            }
        );
    }

    #[test]
    fn lists_and_mappings_of_plain_values_say_what_they_hold() {
        let form = form_of(
            &json!({}),
            &json!({
                "names": { "type": "array", "items": { "type": "string" } },
                "images": { "type": "object", "additionalProperties": { "type": "string" } },
            }),
        );
        assert_eq!(
            setting(&form, "names").kind,
            SettingKind::List(Box::new(SettingKind::Text))
        );
        assert_eq!(
            setting(&form, "images").kind,
            SettingKind::Mapping(Box::new(SettingKind::Text))
        );
    }

    #[test]
    fn what_has_no_plain_kind_is_raw() {
        let form = form_of(
            &json!({}),
            &json!({
                "rows": { "oneOf": [{ "type": "integer" }, { "const": "auto" }] },
                "devices": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object", "properties": { "tap": { "type": "boolean" } },
                    },
                },
                "cells": { "type": "array", "items": { "type": "object", "properties": { "x": { "type": "integer" } } } },
            }),
        );
        for key in ["rows", "devices", "cells"] {
            assert_eq!(setting(&form, key).kind, SettingKind::Raw, "{key}");
        }
    }

    #[test]
    fn a_reference_to_nothing_is_refused() {
        let error = Form::from_schema(&json!({
            "type": "object",
            "properties": { "a": { "$ref": "#/$defs/Missing" } },
        }))
        .expect_err("refused");
        assert_eq!(
            error,
            FormError::UnresolvedReference("#/$defs/Missing".to_owned())
        );
    }

    #[test]
    fn a_schema_with_no_settings_is_refused() {
        let error = Form::from_schema(&json!({ "type": "object" })).expect_err("refused");
        assert_eq!(error, FormError::NoSettings);
    }
}
