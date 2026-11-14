//! Turning `outputs.toml` into a layout: which outputs are on, how each
//! is set up, where each goes, and which one is primary.

use smithay::output::{Output, Scale};

use super::{OutputControl, Outputs};
use crate::config::{
    OutputEntry, OutputPlan, OutputProfile, OutputScale, OutputSettings, OutputTransform,
};

/// What the settings make of the connected outputs.
#[derive(Debug)]
pub(super) struct Arrangement<'settings> {
    /// The outputs on, each with the entry it follows, in the order found.
    on: Vec<(Output, Option<&'settings OutputEntry>)>,
    /// The outputs off.
    off: Vec<Output>,
}

impl<'settings> Arrangement<'settings> {
    /// Plans `connected` under `settings`, with `profile` applied: each
    /// follows its entry, or is off. If every output would be off, the
    /// first stays on.
    pub(super) fn plan(
        connected: &[Output],
        settings: &'settings OutputSettings,
        profile: Option<&'settings OutputProfile>,
    ) -> Self {
        let mut on = Vec::new();
        let mut off = Vec::new();
        for output in connected {
            let identity = Outputs::identity(output);
            match settings.plan(profile, identity.as_identity()) {
                OutputPlan::Follows(entry) if !entry.enabled => off.push(output.clone()),
                OutputPlan::Follows(entry) => on.push((output.clone(), Some(entry))),
                OutputPlan::Unlisted => on.push((output.clone(), None)),
                OutputPlan::Off => off.push(output.clone()),
            }
        }
        if on.is_empty()
            && let Some(first) = connected.first()
        {
            tracing::warn!("outputs.toml turns every output off; the first stays on");
            off.retain(|output| output != first);
            on.push((first.clone(), None));
        }
        Self { on, off }
    }

    /// Has `backend` turn the outputs on and off and set their modes and
    /// transforms, then sets their scales and positions: those an entry
    /// places first, the rest side by side to their right.
    pub(super) fn apply(&self, backend: &mut impl OutputControl) {
        for output in &self.off {
            backend.set_enabled(output, false);
        }
        for (output, entry) in &self.on {
            backend.set_enabled(output, true);
            backend.set_mode(output, entry.and_then(|entry| entry.mode));
            let transform = entry.map_or(OutputTransform::Normal, |entry| entry.transform);
            backend.set_transform(output, OutputTransform::to_smithay(transform));
            let scale = entry
                .and_then(|entry| entry.scale)
                .map_or(1.0, OutputScale::get);
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
        }
        let mut right = 0;
        for (output, entry) in &self.on {
            if let Some([x, y]) = entry.and_then(|entry| entry.position) {
                output.change_current_state(None, None, None, Some((x, y).into()));
                let area = Outputs::geometry(output);
                right = right.max(area.loc.x + area.size.w);
            }
        }
        let unplaced = self
            .on
            .iter()
            .filter(|(_, entry)| entry.and_then(|entry| entry.position).is_none())
            .map(|(output, _)| output);
        Outputs::lay_out_in_a_row(right, unplaced);
    }

    /// Returns the primary output and the others on. The primary one is
    /// the output `chosen` names, if it is on; else the first of
    /// `profile`'s candidates on; else one whose entry says `primary`;
    /// else Fluxbox's head 1, the leftmost, then topmost.
    pub(super) fn into_layout(
        self,
        chosen: Option<&str>,
        profile: Option<&OutputProfile>,
    ) -> (Output, Vec<Output>) {
        let identities: Vec<_> = self
            .on
            .iter()
            .map(|(output, _)| Outputs::identity(output))
            .collect();
        let borrowed: Vec<_> = identities
            .iter()
            .map(|identity| identity.as_identity())
            .collect();
        let candidate = profile
            .and_then(|profile| profile.primary_candidates(&borrowed).first().copied())
            .map(|identity| identity.connector.to_owned());
        let named = |name: &str| self.on.iter().position(|(output, _)| output.name() == name);
        let index = chosen
            .and_then(named)
            .or_else(|| candidate.as_deref().and_then(named))
            .or_else(|| {
                self.on
                    .iter()
                    .position(|(_, entry)| entry.is_some_and(|entry| entry.primary))
            })
            .or_else(|| {
                self.on
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, (output, _))| Outputs::head_order(output))
                    .map(|(index, _)| index)
            })
            .unwrap_or(0);
        let mut on: Vec<Output> = self.on.into_iter().map(|(output, _)| output).collect();
        let primary = on.remove(index);
        (primary, on)
    }
}
