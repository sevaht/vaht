//! Windows going back to their monitor when it is plugged in again.
//!
//! When an output leaves, its windows are brought onto the outputs left
//! ([`crate::window_manager::OutputAreas::corner_shown`]). [`OutputMemory`]
//! remembers, for each, the output it was on (by name) and where on it,
//! and where it was brought. When an output of that name comes back, each
//! window still where it was brought goes back where it was; one the user
//! moved since stays where the user put it.
//!
//! It is plain values over any window key, so tests follow it alone.

use smithay::utils::{Logical, Point, Rectangle};

/// A window's area, in global coordinates.
type Area = Rectangle<i32, Logical>;

/// What is remembered of one window.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Remembered<Key> {
    /// The window.
    window: Key,
    /// The output it was on.
    output: String,
    /// Where its corner was, from the output's corner.
    offset: Point<i32, Logical>,
    /// Where it was brought when the output left, once known.
    brought_to: Option<Area>,
}

/// The windows of outputs that left.
#[derive(Debug, Clone)]
pub(crate) struct OutputMemory<Key> {
    /// Each window remembered.
    remembered: Vec<Remembered<Key>>,
}

impl<Key> Default for OutputMemory<Key> {
    fn default() -> Self {
        Self {
            remembered: Vec::new(),
        }
    }
}

impl<Key: Copy + PartialEq> OutputMemory<Key> {
    /// Remembers the `windows` (each with its area) on the output `name`,
    /// whose corner is at `corner`, which is leaving.
    pub(crate) fn output_leaving(
        &mut self,
        name: &str,
        corner: Point<i32, Logical>,
        windows: impl IntoIterator<Item = (Key, Area)>,
    ) {
        for (window, area) in windows {
            self.remembered.retain(|known| known.window != window);
            self.remembered.push(Remembered {
                window,
                output: name.to_owned(),
                offset: area.loc - corner,
                brought_to: None,
            });
        }
    }

    /// Notes where the windows just remembered were brought: `area_of`
    /// gives each window's area now, or none if it is gone.
    pub(crate) fn note_brought(&mut self, area_of: impl Fn(Key) -> Option<Area>) {
        self.remembered.retain_mut(|known| {
            if known.brought_to.is_none() {
                known.brought_to = area_of(known.window);
            }
            known.brought_to.is_some()
        });
    }

    /// Returns where each window of the output `name`, back with its
    /// corner at `corner`, goes: those still where they were brought, with
    /// their corner's place on the output. `area_of` gives each window's
    /// area now, or none if it is gone. The output's windows are
    /// forgotten.
    pub(crate) fn output_back(
        &mut self,
        name: &str,
        corner: Point<i32, Logical>,
        area_of: impl Fn(Key) -> Option<Area>,
    ) -> Vec<(Key, Point<i32, Logical>)> {
        let (back, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.remembered)
            .into_iter()
            .partition(|known| known.output == name);
        self.remembered = kept;
        back.into_iter()
            .filter(|known| {
                known
                    .brought_to
                    .is_some_and(|brought_to| area_of(known.window) == Some(brought_to))
            })
            .map(|known| (known.window, corner + known.offset))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100×100 area at `(x, y)`.
    fn area(x: i32, y: i32) -> Area {
        Rectangle::new((x, y).into(), (100, 100).into())
    }

    #[test]
    fn a_window_goes_back_where_it_was_unless_moved_since() {
        let mut memory = OutputMemory::default();
        let right = Point::from((800, 0));
        memory.output_leaving("RIGHT", right, [(1, area(900, 50)), (2, area(1000, 200))]);
        // Both were brought onto the left output.
        let now = |window| match window {
            1 => Some(area(100, 50)),
            2 => Some(area(200, 200)),
            _ => None,
        };
        memory.note_brought(now);
        // The user moved window 2 since.
        let later = |window| match window {
            1 => Some(area(100, 50)),
            2 => Some(area(300, 300)),
            _ => None,
        };
        assert_eq!(
            memory.output_back("RIGHT", right, later),
            [(1, Point::from((900, 50)))]
        );
        assert_eq!(
            memory.output_back("RIGHT", right, later),
            [],
            "forgotten once back"
        );
    }

    #[test]
    fn a_window_goes_back_to_the_output_wherever_it_now_is() {
        let mut memory = OutputMemory::default();
        memory.output_leaving("RIGHT", (800, 0).into(), [(1, area(900, 50))]);
        memory.note_brought(|_| Some(area(100, 50)));
        let back = memory.output_back("RIGHT", (1920, 0).into(), |_| Some(area(100, 50)));
        assert_eq!(back, [(1, Point::from((2020, 50)))], "the same place on it");
    }

    #[test]
    fn a_window_gone_is_forgotten_and_another_output_returns_nothing() {
        let mut memory = OutputMemory::default();
        memory.output_leaving("RIGHT", (800, 0).into(), [(1, area(900, 50))]);
        memory.note_brought(|_| None);
        assert_eq!(memory.output_back("RIGHT", (800, 0).into(), |_| None), []);
        memory.output_leaving("RIGHT", (800, 0).into(), [(2, area(900, 50))]);
        memory.note_brought(|_| Some(area(0, 0)));
        assert_eq!(
            memory.output_back("LEFT", (0, 0).into(), |_| Some(area(0, 0))),
            []
        );
    }
}
