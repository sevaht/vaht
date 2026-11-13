//! Converting fluxland's `session.output.<connector>.*` resources to
//! `outputs.toml` (02 §9), and finding the output the toolbar's
//! `onhead` names.
//!
//! fluxland numbers heads from 1, left to right and then top to bottom by
//! where the outputs are, counting only those on. vaht's primary output
//! is head 1 unless an entry says otherwise, so `onhead` 1 needs nothing;
//! a later head becomes `[taskbar] output`, when the positions in `init`
//! say which output it is.

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

use crate::report::{Reason, Report};
use crate::resources::Resources;

/// The prefix of fluxland's output resources.
const PREFIX: &str = "session.output.";

/// One output's resources, being converted.
#[derive(Debug)]
struct OutputBlock {
    /// Its connector's name.
    connector: String,
    /// Its `[[output]]` entry so far.
    entry: Table,
    /// Whether it is on: not turned off.
    is_enabled: bool,
    /// Where it is, if `init` says.
    position: Option<(i64, i64)>,
}

/// fluxland's output resources, converted.
#[derive(Debug, Default)]
pub(crate) struct OutputResources {
    /// Each output named, in the order first named.
    blocks: Vec<OutputBlock>,
}

impl OutputResources {
    /// Converts the `session.output.*` resources in `resources`, noting in
    /// `report` what vaht lacks.
    pub(crate) fn convert(resources: &mut Resources, report: &mut Report) -> Self {
        let mut converted = Self::default();
        let names = resources.names_with_prefix(PREFIX);
        for name in names {
            let Some((connector, field)) = name
                .strip_prefix(PREFIX)
                .and_then(|rest| rest.rsplit_once('.'))
            else {
                continue;
            };
            let Some(resource) = resources.get(&name).cloned() else {
                continue;
            };
            let block = converted.block(connector);
            if !block.convert(field, &resource.value) {
                report.note(
                    resources.path(),
                    resource.line,
                    Reason::NotYetInVaht,
                    format!("{} = {}", resource.name, resource.value),
                );
            }
        }
        converted
    }

    /// Returns `outputs.toml`, if `init` set any output up.
    pub(crate) fn document(&self) -> Option<DocumentMut> {
        if self.blocks.is_empty() {
            return None;
        }
        let mut entries = ArrayOfTables::new();
        for block in &self.blocks {
            let mut entry = Table::new();
            let mut matcher = InlineTable::new();
            matcher.insert("connector", Value::from(block.connector.as_str()));
            entry.insert("match", Item::Value(Value::InlineTable(matcher)));
            for (key, item) in &block.entry {
                entry.insert(key, item.clone());
            }
            entries.push(entry);
        }
        let mut document = DocumentMut::new();
        document.insert("output", Item::ArrayOfTables(entries));
        Some(document)
    }

    /// Returns the connector of fluxland's head `head` (from 1): the output
    /// on that many places from the left, then top; none if some output on
    /// has no position in `init`, or there are not that many.
    pub(crate) fn head(&self, head: usize) -> Option<&str> {
        let mut heads: Vec<(i64, i64, &str)> = self
            .blocks
            .iter()
            .filter(|block| block.is_enabled)
            .map(|block| {
                block
                    .position
                    .map(|(x, y)| (x, y, block.connector.as_str()))
            })
            .collect::<Option<_>>()?;
        heads.sort_unstable();
        heads
            .get(head.checked_sub(1)?)
            .map(|(_, _, connector)| *connector)
    }

    /// Returns the block of the output on `connector`, adding it if new.
    fn block(&mut self, connector: &str) -> &mut OutputBlock {
        let index = self
            .blocks
            .iter()
            .position(|block| block.connector == connector)
            .unwrap_or_else(|| {
                self.blocks.push(OutputBlock {
                    connector: connector.to_owned(),
                    entry: Table::new(),
                    is_enabled: true,
                    position: None,
                });
                self.blocks.len() - 1
            });
        &mut self.blocks[index]
    }
}

impl OutputBlock {
    /// fluxland's transforms, which vaht names alike.
    const TRANSFORMS: [&str; 8] = [
        "normal",
        "90",
        "180",
        "270",
        "flipped",
        "flipped-90",
        "flipped-180",
        "flipped-270",
    ];

    /// Converts the resource `field` with `value` into the entry; returns
    /// whether vaht could.
    fn convert(&mut self, field: &str, value: &str) -> bool {
        let flag = || match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" | "on" => Some(true),
            "false" | "no" | "0" | "off" => Some(false),
            _ => None,
        };
        match field.to_ascii_lowercase().as_str() {
            "enabled" => match flag() {
                Some(true) => true,
                Some(false) => {
                    self.is_enabled = false;
                    self.entry.insert("enabled", toml_edit::value(false));
                    true
                }
                None => false,
            },
            "mode" => {
                self.entry.insert("mode", toml_edit::value(value));
                true
            }
            "scale" => match value.parse::<f64>() {
                Ok(scale) => {
                    self.entry.insert("scale", toml_edit::value(scale));
                    true
                }
                Err(_) => false,
            },
            "position" => {
                let position = value.split_once(',').and_then(|(x, y)| {
                    Some((x.trim().parse::<i64>().ok()?, y.trim().parse::<i64>().ok()?))
                });
                let Some((x, y)) = position else {
                    return false;
                };
                self.position = Some((x, y));
                self.entry.insert(
                    "position",
                    toml_edit::value(Value::Array([x, y].into_iter().collect())),
                );
                true
            }
            "transform" => {
                let lower = value.to_ascii_lowercase();
                if !Self::TRANSFORMS.contains(&lower.as_str()) {
                    return false;
                }
                self.entry.insert("transform", toml_edit::value(lower));
                true
            }
            "adaptivesync" => match flag() {
                Some(is_on) => {
                    let vrr = if is_on { "on" } else { "off" };
                    self.entry.insert("vrr", toml_edit::value(vrr));
                    true
                }
                None => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// My fluxland outputs: two side by side, the laptop's panel off.
    const INIT: &str = "\
session.output.HDMI-A-1.mode:\t1920x1080@60
session.output.HDMI-A-1.position:\t0,0
session.output.DP-2.mode:\t1920x1080@60
session.output.DP-2.position:\t1920,0
session.output.eDP-1.enabled:\tfalse
";

    #[test]
    fn my_outputs_convert() {
        let mut resources = Resources::parse(Path::new("init"), INIT);
        let mut report = Report::default();
        let outputs = OutputResources::convert(&mut resources, &mut report);
        assert!(report.notes().is_empty(), "{report}");
        assert_eq!(resources.unused().count(), 0);
        assert_eq!(
            outputs.document().expect("outputs are set up").to_string(),
            "[[output]]\nmatch = { connector = \"HDMI-A-1\" }\nmode = \"1920x1080@60\"\n\
             position = [0, 0]\n\n\
             [[output]]\nmatch = { connector = \"DP-2\" }\nmode = \"1920x1080@60\"\n\
             position = [1920, 0]\n\n\
             [[output]]\nmatch = { connector = \"eDP-1\" }\nenabled = false\n"
        );
        assert_eq!(outputs.head(1), Some("HDMI-A-1"));
        assert_eq!(outputs.head(2), Some("DP-2"));
        assert_eq!(outputs.head(3), None, "the panel is off: no third head");
    }

    #[test]
    fn heads_are_unknown_without_every_position() {
        let mut resources = Resources::parse(
            Path::new("init"),
            "session.output.DP-1.position: 0,0\nsession.output.DP-2.scale: 2\n",
        );
        let outputs = OutputResources::convert(&mut resources, &mut Report::default());
        assert_eq!(outputs.head(1), None);
    }

    #[test]
    fn what_vaht_lacks_is_reported() {
        let mut resources = Resources::parse(
            Path::new("init"),
            "session.output.DP-1.transform: sideways\nsession.output.DP-1.gamma: 2\n",
        );
        let mut report = Report::default();
        let _ = OutputResources::convert(&mut resources, &mut report);
        assert_eq!(report.notes().len(), 2, "{report}");
    }
}
