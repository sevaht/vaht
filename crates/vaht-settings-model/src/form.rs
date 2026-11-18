//! The form: the settings as sections of values to fill in.
//!
//! [`Form::from_schema`] reads the JSON Schema vaht serves. A struct becomes
//! a [`Section`], and each leaf a [`Setting`] with the kind of value it takes
//! ([`SettingKind`]), the words to call it, its documentation, its default
//! and whether it may be unset. A value that may be written several ways (a
//! number or the word "auto") is [`SettingKind::Alternatives`], and a table of
//! tables by name (`input.device`) is [`SettingKind::Scopes`]. What the schema
//! cannot say as either (a list of tables) is [`SettingKind::Raw`], until a
//! purpose-built editor takes it over.

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
    /// A value written one of several ways: a number or the word `auto`,
    /// yes, no or `any`, a text or a pattern.
    Alternatives(Vec<Alternative>),
    /// Tables of one shape by name, which the user adds and removes (a
    /// device's name): the shape, with `*` where the name goes.
    Scopes(Box<Section>),
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

impl SettingKind {
    /// Returns a value of this kind to start from when a setting that was
    /// unset is first set: the least it may be, the first word, or empty.
    #[must_use]
    pub fn starting_value(&self) -> Value {
        match self {
            Self::Toggle => Value::Bool(false),
            Self::Whole { minimum, .. } => Value::from(minimum.unwrap_or(0)),
            Self::Alternatives(alternatives) => alternatives
                .first()
                .map_or(Value::Null, Alternative::starting_value),
            Self::Decimal { minimum, .. } => serde_json::Number::from_f64(minimum.unwrap_or(0.0))
                .map_or(Value::Null, Value::Number),
            Self::Text => Value::String(String::new()),
            Self::Choice(choices) => Value::String(
                choices
                    .first()
                    .map(|choice| choice.value.clone())
                    .unwrap_or_default(),
            ),
            Self::List(_) => Value::Array(Vec::new()),
            Self::Mapping(_) | Self::Scopes(_) | Self::Raw => Value::Object(serde_json::Map::new()),
        }
    }

    /// Returns whether `value` is of this kind (not whether it is within
    /// the limits).
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Toggle => value.is_boolean(),
            Self::Whole { .. } | Self::Decimal { .. } => value.is_number(),
            Self::Text | Self::Choice(_) => value.is_string(),
            Self::Alternatives(alternatives) => {
                alternatives.iter().any(|alternative| alternative.is(value))
            }
            Self::List(_) => value.is_array(),
            Self::Mapping(_) | Self::Scopes(_) | Self::Raw => value.is_object(),
        }
    }

    /// Returns what the word or form `value` is, from the setting's
    /// documentation, if it says.
    #[must_use]
    pub fn note(&self, value: &Value) -> Option<&str> {
        let text = match self {
            Self::Choice(choices) => choices
                .iter()
                .find(|choice| value.as_str() == Some(choice.value.as_str()))
                .map(|choice| choice.description.as_str()),
            Self::Alternatives(alternatives) => alternatives
                .iter()
                .find(|alternative| alternative.is(value))
                .map(|alternative| alternative.description.as_str()),
            Self::Toggle
            | Self::Whole { .. }
            | Self::Decimal { .. }
            | Self::Text
            | Self::List(_)
            | Self::Mapping(_)
            | Self::Scopes(_)
            | Self::Raw => None,
        };
        text.filter(|text| !text.is_empty())
    }
}

/// One way a [`SettingKind::Alternatives`] value may be written.
#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    /// What the window calls it.
    pub label: String,
    /// What it means, from the setting's documentation; may be empty.
    pub description: String,
    /// What it is.
    pub shape: Shape,
    /// The value to start from when this way is chosen, if the schema
    /// suggests one (its `default`); else the least it may be.
    pub suggested: Option<Value>,
}

/// What an [`Alternative`] is.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// Exactly this value: a word, yes or no.
    Exact(Value),
    /// A value of this kind.
    Value(SettingKind),
    /// A table with this one key, holding a value of this kind
    /// (`{ max = 300 }`).
    Field {
        /// The key.
        key: String,
        /// What it holds.
        kind: SettingKind,
    },
}

impl Alternative {
    /// Returns whether `value` is written this way.
    #[must_use]
    pub fn is(&self, value: &Value) -> bool {
        match &self.shape {
            Shape::Exact(exact) => exact == value,
            Shape::Value(kind) => kind.accepts(value),
            Shape::Field { key, .. } => value.get(key).is_some(),
        }
    }

    /// Returns a value written this way to start from.
    #[must_use]
    pub fn starting_value(&self) -> Value {
        if let Some(suggested) = &self.suggested {
            return suggested.clone();
        }
        match &self.shape {
            Shape::Exact(exact) => exact.clone(),
            Shape::Value(kind) => kind.starting_value(),
            Shape::Field { key, kind } => {
                let mut table = serde_json::Map::new();
                table.insert(key.clone(), kind.starting_value());
                Value::Object(table)
            }
        }
    }
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
    /// The heading it is shown under on its page (`x-vaht-group`), if the
    /// schema groups the settings; neighbours with the same group are
    /// shown together, in the order the schema declares them.
    pub group: Option<String>,
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
    /// Returns this section as the one called `name`: the part of every
    /// path where a [`SettingKind::Scopes`] shape has its `*`.
    #[must_use]
    pub fn named(&self, name: &str) -> Self {
        let index = self.path.len().saturating_sub(1);
        let mut section = self.with_part(index, name);
        name.clone_into(&mut section.label);
        section
    }

    /// Returns this section with part `index` of every path `name`.
    fn with_part(&self, index: usize, name: &str) -> Self {
        let swap = |parts: &[String]| {
            let mut parts = parts.to_vec();
            if let Some(part) = parts.get_mut(index) {
                name.clone_into(part);
            }
            parts
        };
        Self {
            path: swap(&self.path),
            label: self.label.clone(),
            description: self.description.clone(),
            settings: self
                .settings
                .iter()
                .map(|setting| Setting {
                    path: ConfigPath::from_parts(swap(setting.path.parts()))
                        .unwrap_or_else(|| setting.path.clone()),
                    ..setting.clone()
                })
                .collect(),
            sections: self
                .sections
                .iter()
                .map(|section| section.with_part(index, name))
                .collect(),
        }
    }

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
        let root = reader.section(&[], schema, schema, 0)?;
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
    /// How many tables that may be left out may hold each other.
    const OPTIONAL_DEPTH: usize = 1;

    /// Reads `schema` (a struct's, at `path`) as a section; `outer` is the
    /// node that referred to it, whose documentation wins.
    fn section(
        &self,
        path: &[String],
        schema: &'schema Value,
        outer: &'schema Value,
        optional_depth: usize,
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
            if Self::is_struct(resolved.schema) {
                // A table that may be left out may hold itself (a matcher's
                // `not`): one level of it is shown, the next is left to the file.
                if resolved.is_optional && optional_depth >= Self::OPTIONAL_DEPTH {
                    continue;
                }
                let depth = optional_depth + usize::from(resolved.is_optional);
                section
                    .sections
                    .push(self.section(&property_path, property, property, depth)?);
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
        let (key_label, unit) = path
            .last()
            .map_or_else(|| (String::new(), None), |key| label::of_key(key));
        let label = Self::title_of(property, resolved.schema).unwrap_or(key_label);
        let kind = self
            .scopes_of(&path, resolved.schema)
            .unwrap_or_else(|| self.kind_of(resolved.schema));
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
            kind,
            is_optional: resolved.is_optional,
            group: Self::extension(property, resolved.schema, "x-vaht-group")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    /// Returns what `outer` or, failing that, `inner` says under the
    /// extension keyword `name`.
    fn extension<'node>(
        outer: &'node Value,
        inner: &'node Value,
        name: &str,
    ) -> Option<&'node Value> {
        outer.get(name).or_else(|| inner.get(name))
    }

    /// Returns what `outer` or, failing that, `inner` says it is for.
    fn title_of(outer: &Value, inner: &Value) -> Option<String> {
        [outer, inner]
            .into_iter()
            .find_map(|node| node.get("title").and_then(Value::as_str))
            .map(str::to_owned)
    }

    /// Returns the kind of a table of tables by name, if `schema` is one:
    /// `path` is where the names go.
    fn scopes_of(&self, path: &[String], schema: &'schema Value) -> Option<SettingKind> {
        if Self::is_struct(schema) || schema.get("type") != Some(&Value::from("object")) {
            return None;
        }
        let items = Self::values_of(schema)?;
        let resolved = self.resolve(items).ok()?;
        if !Self::is_struct(resolved.schema) {
            return None;
        }
        let mut scope_path = path.to_vec();
        scope_path.push("*".to_owned());
        let section = self.section(&scope_path, items, items, 0).ok()?;
        Some(SettingKind::Scopes(Box::new(section)))
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

    /// Returns the schema of the values of a table by name: its
    /// `additionalProperties`, or its one pattern of names.
    fn values_of(schema: &Value) -> Option<&Value> {
        schema
            .get("additionalProperties")
            .filter(|values| values.is_object())
            .or_else(|| {
                let patterns = schema.get("patternProperties")?.as_object()?;
                (patterns.len() == 1).then(|| patterns.values().next())?
            })
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
                    is_optional: is_optional || Self::type_names(schema).any(|name| name == "null"),
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

    /// Returns the names `schema`'s `type` lists (it is a name or an array
    /// of them).
    fn type_names(schema: &Value) -> impl Iterator<Item = &str> {
        let value = schema.get("type");
        value.and_then(Value::as_str).into_iter().chain(
            value
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
    }

    /// Returns the kind of value `schema` describes.
    fn kind_of(&self, schema: &'schema Value) -> SettingKind {
        if let Some(choices) = Self::choices_of(schema) {
            return SettingKind::Choice(choices);
        }
        if let Some(alternatives) = self.alternatives_of(schema) {
            return SettingKind::Alternatives(alternatives);
        }
        match Self::type_names(schema).find(|name| *name != "null") {
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
            Some("object") if !Self::is_struct(schema) => {
                self.container(Self::values_of(schema), SettingKind::Mapping)
            }
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

    /// Returns the ways `schema`, a `oneOf`, says a value is written, if
    /// each is one the window can offer.
    fn alternatives_of(&self, schema: &'schema Value) -> Option<Vec<Alternative>> {
        let members = schema.get("oneOf")?.as_array()?;
        let mut alternatives = Vec::new();
        for member in members {
            alternatives.extend(self.alternative_of(member)?);
        }
        (alternatives.len() > 1).then_some(alternatives)
    }

    /// Returns the ways `member`, one of a `oneOf`, is written: one, or
    /// two for a yes or no.
    fn alternative_of(&self, member: &'schema Value) -> Option<Vec<Alternative>> {
        let member = self.resolve(member).ok()?.schema;
        let description = member
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let title = member.get("title").and_then(Value::as_str);
        let suggested = member.get("default").cloned();
        let alternative = |label: String, shape: Shape| Alternative {
            label: title.map_or(label, str::to_owned),
            description: description.clone(),
            shape,
            suggested: suggested.clone(),
        };
        if let Some(exact) = member.get("const") {
            let label = exact.as_str().unwrap_or_default().to_owned();
            return Some(vec![alternative(label, Shape::Exact(exact.clone()))]);
        }
        if let Some(properties) = member
            .get("properties")
            .and_then(Value::as_object)
            .filter(|properties| properties.len() == 1)
        {
            let (key, property) = properties.iter().next()?;
            let kind = self.kind_of(self.resolve(property).ok()?.schema);
            if kind == SettingKind::Raw {
                return None;
            }
            let shape = Shape::Field {
                key: key.clone(),
                kind,
            };
            return Some(vec![alternative(label::of_key(key).0, shape)]);
        }
        match self.kind_of(member) {
            SettingKind::Toggle => Some(vec![
                Alternative {
                    label: "yes".to_owned(),
                    description: description.clone(),
                    shape: Shape::Exact(Value::Bool(true)),
                    suggested: None,
                },
                Alternative {
                    label: "no".to_owned(),
                    description,
                    shape: Shape::Exact(Value::Bool(false)),
                    suggested: None,
                },
            ]),
            kind @ (SettingKind::Whole { .. } | SettingKind::Decimal { .. }) => {
                Some(vec![alternative("a number".to_owned(), Shape::Value(kind))])
            }
            kind @ SettingKind::Text => {
                Some(vec![alternative("text".to_owned(), Shape::Value(kind))])
            }
            // Words are written as they are, and nothing else nests.
            SettingKind::Raw
            | SettingKind::Choice(_)
            | SettingKind::Alternatives(_)
            | SettingKind::List(_)
            | SettingKind::Mapping(_)
            | SettingKind::Scopes(_) => None,
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

    /// Returns the alternatives of the setting `key`.
    fn alternatives(form: &Form, key: &str) -> Vec<Alternative> {
        let SettingKind::Alternatives(alternatives) = &setting(form, key).kind else {
            panic!("{key} is written several ways");
        };
        alternatives.clone()
    }

    #[test]
    fn a_number_or_a_word_is_written_either_way() {
        let form = form_of(
            &json!({}),
            &json!({
                "rows": { "oneOf": [
                    { "type": "integer", "minimum": 1 },
                    { "const": "auto", "title": "Automatic", "description": "As many as needed." },
                ] },
            }),
        );
        let alternatives = alternatives(&form, "rows");
        assert_eq!(alternatives.len(), 2);
        assert_eq!(alternatives[0].label, "a number");
        assert_eq!(alternatives[1].label, "Automatic");
        assert!(alternatives[0].is(&json!(3)));
        assert!(alternatives[1].is(&json!("auto")));
        assert!(!alternatives[1].is(&json!("fill")));
        let kind = &setting(&form, "rows").kind;
        assert_eq!(kind.note(&json!("auto")), Some("As many as needed."));
        assert_eq!(kind.note(&json!(3)), None);
        assert_eq!(kind.starting_value(), json!(1));
    }

    #[test]
    fn a_table_with_one_key_is_an_alternative_to_a_word() {
        let form = form_of(
            &json!({}),
            &json!({
                "width": { "oneOf": [
                    { "const": "fill" },
                    {
                        "type": "object", "required": ["max"],
                        "properties": { "max": { "type": "integer", "minimum": 1 } },
                    },
                ] },
            }),
        );
        let alternatives = alternatives(&form, "width");
        assert_eq!(alternatives[1].label, "Max");
        assert!(alternatives[1].is(&json!({ "max": 300 })));
        assert_eq!(alternatives[1].starting_value(), json!({ "max": 1 }));
    }

    #[test]
    fn a_way_starts_from_the_value_the_schema_suggests() {
        let form = form_of(
            &json!({}),
            &json!({
                "width": { "oneOf": [
                    { "type": "integer", "title": "Pixels", "default": 200 },
                    {
                        "type": "object", "required": ["percent"],
                        "properties": { "percent": { "type": "integer", "minimum": 1 } },
                        "default": { "percent": 15 },
                    },
                ] },
            }),
        );
        let alternatives = alternatives(&form, "width");
        assert_eq!(alternatives[0].starting_value(), json!(200));
        assert_eq!(alternatives[1].starting_value(), json!({ "percent": 15 }));
    }

    #[test]
    fn a_yes_or_no_beside_a_word_is_three_ways() {
        let form = form_of(
            &json!({}),
            &json!({ "minimized": { "oneOf": [{ "type": "boolean" }, { "const": "any" }] } }),
        );
        let labels: Vec<String> = alternatives(&form, "minimized")
            .into_iter()
            .map(|alternative| alternative.label)
            .collect();
        assert_eq!(labels, ["yes", "no", "any"]);
    }

    #[test]
    fn a_table_of_tables_by_name_is_scopes_to_add_to() {
        let form = form_of(
            &json!({}),
            &json!({
                "device": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object", "properties": { "tap": { "type": "boolean" } },
                    },
                },
            }),
        );
        let SettingKind::Scopes(scope) = &setting(&form, "device").kind else {
            panic!("scopes");
        };
        assert_eq!(scope.path, ["device", "*"]);
        let named = scope.named("My Mouse");
        assert_eq!(named.label, "My Mouse");
        assert_eq!(
            named.settings[0].path.parts(),
            ["device", "My Mouse", "tap"]
        );
    }

    #[test]
    fn a_table_that_may_hold_itself_is_shown_one_level_deep() {
        let matcher = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "not": { "anyOf": [{ "$ref": "#/$defs/Matcher" }, { "type": "null" }] },
            },
        });
        let form = Form::from_schema(&json!({
            "type": "object",
            "properties": { "show": { "$ref": "#/$defs/Matcher" } },
            "$defs": { "Matcher": matcher },
        }))
        .expect("a form");
        let paths: Vec<String> = form
            .settings()
            .into_iter()
            .map(|setting| setting.path.to_string())
            .collect();
        assert_eq!(paths, ["show.title", "show.not.title"]);
    }

    #[test]
    fn a_table_by_names_of_one_pattern_is_a_mapping() {
        let form = form_of(
            &json!({}),
            &json!({
                "images": {
                    "type": "object",
                    "patternProperties": { "^\\d+$": { "type": "string" } },
                },
            }),
        );
        assert_eq!(
            setting(&form, "images").kind,
            SettingKind::Mapping(Box::new(SettingKind::Text))
        );
    }

    #[test]
    fn settings_keep_the_order_the_schema_declares_and_name_their_group() {
        let schema: Value = serde_json::from_str(
            r#"{ "type": "object", "properties": {
                "zeta":  { "type": "boolean", "x-vaht-group": "Basics" },
                "alpha": { "type": "boolean", "x-vaht-group": "Basics" },
                "mid":   { "type": "boolean", "x-vaht-group": "More" },
                "none":  { "type": "boolean" }
            } }"#,
        )
        .expect("JSON");
        let form = Form::from_schema(&schema).expect("a form");
        let listed: Vec<(String, Option<String>)> = form
            .settings()
            .into_iter()
            .map(|setting| (setting.path.to_string(), setting.group.clone()))
            .collect();
        assert_eq!(
            listed,
            [
                ("zeta".to_owned(), Some("Basics".to_owned())),
                ("alpha".to_owned(), Some("Basics".to_owned())),
                ("mid".to_owned(), Some("More".to_owned())),
                ("none".to_owned(), None),
            ],
            "not alphabetical: the schema's order"
        );
    }

    #[test]
    fn a_title_names_a_setting() {
        let form = form_of(
            &json!({}),
            &json!({ "windows": { "type": "string", "title": "Windows listed" } }),
        );
        assert_eq!(setting(&form, "windows").label, "Windows listed");
    }

    #[test]
    fn what_has_no_plain_kind_is_raw() {
        let form = form_of(
            &json!({}),
            &json!({
                "mixed": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "object", "properties": { "x": { "type": "integer" } } } }] },
                "cells": { "type": "array", "items": { "type": "object", "properties": { "x": { "type": "integer" } } } },
            }),
        );
        for key in ["mixed", "cells"] {
            assert_eq!(setting(&form, key).kind, SettingKind::Raw, "{key}");
        }
    }

    #[test]
    fn a_type_that_may_be_null_is_optional() {
        let form = form_of(
            &json!({}),
            &json!({ "style": { "type": ["string", "null"] } }),
        );
        let style = setting(&form, "style");
        assert!(style.is_optional);
        assert_eq!(style.kind, SettingKind::Text);
    }

    #[test]
    fn a_setting_first_set_starts_from_the_least_it_may_be() {
        let kind = |schema: Value| {
            let form = form_of(&json!({}), &json!({ "a": schema }));
            setting(&form, "a").kind.clone()
        };
        assert_eq!(
            kind(json!({ "type": "integer", "minimum": 5 })).starting_value(),
            json!(5)
        );
        assert_eq!(
            kind(json!({ "type": "boolean" })).starting_value(),
            json!(false)
        );
        assert_eq!(
            kind(json!({ "enum": ["x", "y"] })).starting_value(),
            json!("x")
        );
        assert_eq!(
            kind(json!({ "type": "string" })).starting_value(),
            json!("")
        );
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
