//! Fluxbox's `init`: X resources, one `name: value` per line.
//!
//! [`Resources`] reads them and remembers which ones the conversion used,
//! so the rest can be reported rather than silently lost.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One resource and where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resource {
    /// The name, such as `session.screen0.focusModel`.
    pub(crate) name: String,
    /// The value, trimmed.
    pub(crate) value: String,
    /// The line, counting from 1.
    pub(crate) line: usize,
}

/// The resources of an `init` file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Resources {
    /// The file they came from.
    path: PathBuf,
    /// Every resource, in file order; a later duplicate wins.
    entries: Vec<Resource>,
    /// The names the conversion asked for.
    used: BTreeSet<String>,
}

impl Resources {
    /// Parses `text`, read from `path`. Comment lines (`!` or `#`) and
    /// lines without a colon are skipped.
    pub(crate) fn parse(path: &Path, text: &str) -> Self {
        let entries = text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let trimmed = line.trim();
                if trimmed.starts_with('!') || trimmed.starts_with('#') {
                    return None;
                }
                let (name, value) = trimmed.split_once(':')?;
                Some(Resource {
                    name: name.trim().to_owned(),
                    value: value.trim().to_owned(),
                    line: index + 1,
                })
            })
            .collect();
        Self {
            path: path.to_path_buf(),
            entries,
            used: BTreeSet::new(),
        }
    }

    /// Returns the file the resources came from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the resource called `name` (the last one, if repeated), and
    /// marks it used.
    pub(crate) fn get(&mut self, name: &str) -> Option<&Resource> {
        self.used.insert(name.to_owned());
        self.entries
            .iter()
            .rev()
            .find(|resource| resource.name == name)
    }

    /// Returns `name`'s value, if it is set.
    pub(crate) fn value(&mut self, name: &str) -> Option<&str> {
        self.get(name).map(|resource| resource.value.as_str())
    }

    /// Returns the resources the conversion never asked for.
    pub(crate) fn unused(&self) -> impl Iterator<Item = &Resource> {
        self.entries
            .iter()
            .filter(|resource| !self.used.contains(&resource.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_are_read_by_name_and_the_unused_are_known() {
        let mut resources = Resources::parse(
            Path::new("init"),
            "! a comment\nsession.screen0.focusModel:\tClickFocus\nsession.cacheMax: 200\n",
        );
        assert_eq!(
            resources.value("session.screen0.focusModel"),
            Some("ClickFocus")
        );
        let unused: Vec<_> = resources.unused().map(|resource| resource.line).collect();
        assert_eq!(unused, [3]);
    }
}
