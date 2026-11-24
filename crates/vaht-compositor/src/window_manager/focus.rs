//! Keyboard focus: which window has it, the order windows had it in, and
//! cycling through windows as Alt+Tab does.
//!
//! [`Focus`] decides; the compositor carries its decisions out with the
//! seat. Cycling walks the windows in the order they last had focus, without
//! reordering them while it lasts, so holding Alt and pressing Tab again
//! and again visits every window. When the cycle ends (the modifiers are
//! let go), the window it stopped at becomes the most recent.

use super::window::WindowId;

/// Which way a cycle step goes through windows ordered by recency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CycleDirection {
    /// Toward windows that had focus longer ago.
    Forward,
    /// Toward windows that had focus more recently.
    Backward,
}

/// Where a window stood in the order windows had focus in, for another that
/// takes its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusPlace {
    /// How many windows had focus more recently, if it ever had it.
    recency: Option<usize>,
    /// Where it was in the cycle in progress, if one is.
    cycle_position: Option<usize>,
}

/// Which window has keyboard focus, and which had it before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Focus {
    /// The window with focus.
    focused: Option<WindowId>,
    /// Windows by how recently they had focus, most recent first.
    history: Vec<WindowId>,
    /// The cycle in progress, if any.
    cycle: Option<FocusCycle>,
}

/// A walk through windows in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FocusCycle {
    /// The windows walked through, in order.
    windows: Vec<WindowId>,
    /// Where in `windows` the walk is; none before the first step.
    position: Option<usize>,
}

impl Focus {
    /// Returns the window with focus.
    pub(crate) const fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// Returns `windows` ordered by how recently they had focus, most recent
    /// first; windows that never had it come last, in their given order.
    pub(crate) fn by_recency(&self, windows: impl IntoIterator<Item = WindowId>) -> Vec<WindowId> {
        let mut ordered: Vec<WindowId> = windows.into_iter().collect();
        ordered.sort_by_key(|window| {
            self.history
                .iter()
                .position(|known| known == window)
                .unwrap_or(usize::MAX)
        });
        ordered
    }

    /// Returns the window a cycle step in `direction` goes to, among
    /// `windows` ordered by recency, and makes it the cycle's position. The
    /// first step starts a cycle at the focused window.
    pub(crate) fn step(
        &mut self,
        windows: Vec<WindowId>,
        direction: CycleDirection,
    ) -> Option<WindowId> {
        let focused = self.focused;
        let cycle = self.cycle.get_or_insert_with(|| FocusCycle {
            position: focused.and_then(|focused| windows.iter().position(|id| *id == focused)),
            windows,
        });
        let count = cycle.windows.len();
        if count == 0 {
            return None;
        }
        let next = match (cycle.position, direction) {
            (Some(position), CycleDirection::Forward) => (position + 1) % count,
            (Some(position), CycleDirection::Backward) => (position + count - 1) % count,
            (None, CycleDirection::Forward) => 0,
            (None, CycleDirection::Backward) => count - 1,
        };
        cycle.position = Some(next);
        cycle.windows.get(next).copied()
    }

    /// Records that `window` has focus now. Outside a cycle it becomes the
    /// most recent.
    pub(crate) fn set(&mut self, window: Option<WindowId>) {
        self.focused = window;
        if self.cycle.is_none() {
            self.promote_focused();
        }
    }

    /// Ends the cycle in progress, if any; the window it reached becomes the
    /// most recent.
    pub(crate) fn finish_cycle(&mut self) {
        if self.cycle.take().is_some() {
            self.promote_focused();
        }
    }

    /// Returns where `window` stands in the order windows had focus in, and
    /// in the cycle in progress.
    pub(crate) fn place_of(&self, window: WindowId) -> FocusPlace {
        FocusPlace {
            recency: self.history.iter().position(|known| *known == window),
            cycle_position: self
                .cycle
                .as_ref()
                .and_then(|cycle| cycle.windows.iter().position(|known| *known == window)),
        }
    }

    /// Puts `window` where `place` was: as recently as the window it takes
    /// the place of had focus, and in its place in the cycle in progress.
    pub(crate) fn give_place(&mut self, window: WindowId, place: FocusPlace) {
        if let Some(recency) = place.recency {
            self.history.retain(|known| *known != window);
            self.history.insert(recency.min(self.history.len()), window);
        }
        if let (Some(cycle), Some(position)) = (&mut self.cycle, place.cycle_position) {
            cycle.windows.retain(|known| *known != window);
            cycle
                .windows
                .insert(position.min(cycle.windows.len()), window);
        }
    }

    /// Forgets `window`, which is gone.
    pub(crate) fn forget(&mut self, window: WindowId) {
        self.history.retain(|known| *known != window);
        if let Some(cycle) = &mut self.cycle {
            cycle.windows.retain(|known| *known != window);
            cycle.position = None;
        }
        if self.focused == Some(window) {
            self.focused = None;
        }
    }

    /// Makes the focused window the most recent.
    fn promote_focused(&mut self) {
        if let Some(focused) = self.focused {
            self.history.retain(|known| *known != focused);
            self.history.insert(0, focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window ids from raw numbers.
    fn ids(numbers: &[u64]) -> Vec<WindowId> {
        numbers.iter().copied().map(WindowId::new).collect()
    }

    /// Focus having focused windows 1, 2, then 3.
    fn three_focused() -> Focus {
        let mut focus = Focus::default();
        for id in ids(&[1, 2, 3]) {
            focus.set(Some(id));
        }
        focus
    }

    #[test]
    fn windows_order_by_recency() {
        let focus = three_focused();
        assert_eq!(focus.by_recency(ids(&[1, 2, 3, 4])), ids(&[3, 2, 1, 4]));
    }

    #[test]
    fn cycling_visits_every_window_without_reordering_until_it_ends() {
        let mut focus = three_focused();
        let mut visited = Vec::new();
        for _ in 0..3 {
            let window = focus.step(focus.by_recency(ids(&[1, 2, 3])), CycleDirection::Forward);
            focus.set(window);
            visited.extend(window);
        }
        assert_eq!(visited, ids(&[2, 1, 3]), "and around to where it began");
        let stopped_at = focus.step(focus.by_recency(ids(&[1, 2, 3])), CycleDirection::Forward);
        assert_eq!(stopped_at, Some(WindowId::new(2)));
        focus.set(stopped_at);
        focus.finish_cycle();
        assert_eq!(focus.by_recency(ids(&[1, 2, 3])), ids(&[2, 3, 1]));
    }

    #[test]
    fn a_window_that_takes_anothers_place_is_as_recent_as_it_was() {
        let mut focus = three_focused();
        let place = focus.place_of(WindowId::new(2));
        focus.forget(WindowId::new(2));
        assert_eq!(focus.by_recency(ids(&[1, 3, 4])), ids(&[3, 1, 4]));
        focus.give_place(WindowId::new(4), place);
        assert_eq!(focus.by_recency(ids(&[1, 3, 4])), ids(&[3, 4, 1]));
    }

    #[test]
    fn stepping_back_goes_to_the_least_recent() {
        let mut focus = three_focused();
        let windows = focus.by_recency(ids(&[1, 2, 3]));
        assert_eq!(
            focus.step(windows, CycleDirection::Backward),
            Some(WindowId::new(1))
        );
    }

    #[test]
    fn a_closed_window_is_forgotten() {
        let mut focus = three_focused();
        focus.forget(WindowId::new(3));
        assert_eq!(focus.focused(), None);
        assert_eq!(focus.by_recency(ids(&[1, 2])), ids(&[2, 1]));
    }
}
