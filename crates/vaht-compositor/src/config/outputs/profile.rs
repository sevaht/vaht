//! `outputs.toml`'s display profiles and hooks (02 §9): av-output-switcher's
//! video half.
//!
//! A `[[profile]]` is a whole layout: the outputs it lists, each set up as
//! an `[[output]]` entry would be, with every other output off. One is
//! applied by name or by cycling, or by itself when the outputs its
//! `when_connected` lists are there; the hooks are told.

use super::{OutputEntry, OutputIdentity, OutputMatcher};

/// One `[[profile]]`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputProfile {
    /// Its name, which `apply-profile` takes.
    name: String,
    /// The outputs whose being connected applies it by itself; never
    /// without any. When several profiles qualify, the one listing the
    /// most outputs wins.
    #[serde(default)]
    when_connected: Vec<OutputMatcher>,
    /// The outputs that can be primary, in order: the first one on is,
    /// and `cycle-primary-output` steps through them.
    #[serde(default)]
    primary: Vec<OutputMatcher>,
    /// The outputs it turns on and how; the rest are off.
    outputs: Vec<OutputEntry>,
}

/// `outputs.toml`'s `[hooks]`: commands run when outputs change, each
/// given what changed as its one argument.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct OutputHooks {
    /// Run after a profile is applied, with its name.
    pub(crate) profile_applied: Option<String>,
    /// Run after the primary output changes, with its connector.
    pub(crate) primary_output_changed: Option<String>,
}

impl OutputProfile {
    /// Returns its name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the entry `output` follows while this profile is applied;
    /// none if the profile turns it off.
    pub(crate) fn entry_for(&self, output: OutputIdentity<'_>) -> Option<&OutputEntry> {
        self.outputs
            .iter()
            .find(|entry| entry.matcher.matches(output))
    }

    /// Returns whether every output it lists is among `connected`.
    pub(crate) fn fits(&self, connected: &[OutputIdentity<'_>]) -> bool {
        self.outputs.iter().all(|entry| {
            connected
                .iter()
                .any(|output| entry.matcher.matches(*output))
        })
    }

    /// Returns how many outputs its `when_connected` lists, if all of them
    /// are among `connected`; none if it lists none or one is missing.
    pub(crate) fn connected_match(&self, connected: &[OutputIdentity<'_>]) -> Option<usize> {
        let is_all_there = !self.when_connected.is_empty()
            && self
                .when_connected
                .iter()
                .all(|matcher| connected.iter().any(|output| matcher.matches(*output)));
        is_all_there.then_some(self.when_connected.len())
    }

    /// Returns the outputs of `on` that can be primary, in this profile's
    /// order: each candidate's first match, once.
    pub(crate) fn primary_candidates<'output>(
        &self,
        on: &[OutputIdentity<'output>],
    ) -> Vec<OutputIdentity<'output>> {
        let mut candidates: Vec<OutputIdentity<'output>> = Vec::new();
        for matcher in &self.primary {
            if let Some(output) = on.iter().find(|output| matcher.matches(**output))
                && !candidates.contains(output)
            {
                candidates.push(*output);
            }
        }
        candidates
    }
}
