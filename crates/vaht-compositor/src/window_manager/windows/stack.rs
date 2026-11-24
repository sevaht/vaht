//! The stacking order of windows: the single record of what is above what.
//!
//! Windows are stacked in layers, as Fluxbox stacks them: every window of a
//! higher layer is above every window of a lower one, and raising or
//! lowering a window moves it within its own layer only.

use std::fmt;
use std::str::FromStr;

use crate::window_manager::WindowId;

/// A stacking layer, from the top down (Fluxbox's layers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, strum::EnumIter)]
pub(crate) enum StackLayer {
    /// The lowest: under every other window.
    Desktop,
    /// Under normal windows.
    Bottom,
    /// Where windows are unless asked otherwise.
    #[default]
    Normal,
    /// Over normal windows.
    Top,
    /// The taskbar's layer; its windows are under the taskbar.
    Dock,
    /// Over the taskbar too.
    AboveDock,
}

impl StackLayer {
    /// Every layer, its name in vaht's files and its label, top first.
    pub(crate) const ALL: [(Self, &str, &str); 6] = [
        (Self::AboveDock, "above-dock", "Above Dock"),
        (Self::Dock, "dock", "Dock"),
        (Self::Top, "top", "Top"),
        (Self::Normal, "normal", "Normal"),
        (Self::Bottom, "bottom", "Bottom"),
        (Self::Desktop, "desktop", "Desktop"),
    ];

    /// Returns the layer's name in vaht's files.
    pub(crate) fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(layer, _, _)| *layer == self)
            .map_or("normal", |(_, name, _)| name)
    }
}

impl FromStr for StackLayer {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .find(|(_, name, _)| *name == text)
            .map(|(layer, _, _)| *layer)
            .ok_or_else(|| {
                format!(
                    "`{text}` is no layer; they are above-dock, dock, top, normal, bottom and desktop"
                )
            })
    }
}

impl fmt::Display for StackLayer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Windows from bottom to top, each with its layer; the last is drawn above
/// all others. The list stays sorted by layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stack {
    /// Window ids and their layers, bottom first.
    bottom_to_top: Vec<(WindowId, StackLayer)>,
}

impl Stack {
    /// Returns the windows from top to bottom, the order hit-testing uses.
    pub(crate) fn top_down(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.bottom_to_top.iter().rev().map(|(id, _)| *id)
    }

    /// Returns the layer `id` is in; none if it is not stacked.
    pub(crate) fn layer_of(&self, id: WindowId) -> Option<StackLayer> {
        self.bottom_to_top
            .iter()
            .find(|(stacked, _)| *stacked == id)
            .map(|(_, layer)| *layer)
    }

    /// Returns the window directly above `id`, if one is.
    pub(crate) fn above(&self, id: WindowId) -> Option<WindowId> {
        let position = self
            .bottom_to_top
            .iter()
            .position(|(stacked, _)| *stacked == id)?;
        self.bottom_to_top
            .get(position + 1)
            .map(|(above, _)| *above)
    }

    /// Moves `id` to directly below `other`, if both are stacked and in the
    /// same layer; returns whether it moved.
    pub(crate) fn place_below(&mut self, id: WindowId, other: WindowId) -> bool {
        let (Some(layer), Some(other_layer)) = (self.layer_of(id), self.layer_of(other)) else {
            return false;
        };
        if id == other || layer != other_layer {
            return false;
        }
        self.remove(id);
        let Some(position) = self
            .bottom_to_top
            .iter()
            .position(|(stacked, _)| *stacked == other)
        else {
            return false;
        };
        self.bottom_to_top.insert(position, (id, layer));
        true
    }

    /// Places `id` above every other window of the normal layer.
    ///
    /// # Panics
    ///
    /// Panics if `id` is already in the stack: an id enters once, when its
    /// window is created.
    pub(crate) fn push(&mut self, id: WindowId) {
        assert!(
            self.layer_of(id).is_none(),
            "window {id} is already stacked"
        );
        self.insert_on_top(id, StackLayer::Normal);
    }

    /// Takes `id` out of the stack; returns whether it was there.
    pub(crate) fn remove(&mut self, id: WindowId) -> bool {
        let before = self.bottom_to_top.len();
        self.bottom_to_top.retain(|(stacked, _)| *stacked != id);
        self.bottom_to_top.len() != before
    }

    /// Moves `id` below every other window of its layer; returns whether it
    /// was stacked.
    pub(crate) fn lower(&mut self, id: WindowId) -> bool {
        let Some(layer) = self.layer_of(id) else {
            return false;
        };
        self.remove(id);
        let bottom = self
            .bottom_to_top
            .iter()
            .position(|(_, stacked)| *stacked >= layer)
            .unwrap_or(self.bottom_to_top.len());
        self.bottom_to_top.insert(bottom, (id, layer));
        true
    }

    /// Moves `id` above every other window of its layer; returns whether it
    /// was stacked.
    pub(crate) fn raise(&mut self, id: WindowId) -> bool {
        let Some(layer) = self.layer_of(id) else {
            return false;
        };
        self.remove(id);
        self.insert_on_top(id, layer);
        true
    }

    /// Moves `id` to the top of `layer`; returns whether it was stacked.
    pub(crate) fn set_layer(&mut self, id: WindowId, layer: StackLayer) -> bool {
        let was_stacked = self.remove(id);
        if was_stacked {
            self.insert_on_top(id, layer);
        }
        was_stacked
    }

    /// Puts `id` above every window of `layer` and below the higher ones.
    fn insert_on_top(&mut self, id: WindowId, layer: StackLayer) {
        let top = self
            .bottom_to_top
            .iter()
            .position(|(_, stacked)| *stacked > layer)
            .unwrap_or(self.bottom_to_top.len());
        self.bottom_to_top.insert(top, (id, layer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack holding windows 1, 2 and 3, with 3 on top.
    fn three_windows() -> Stack {
        let mut stack = Stack::default();
        for number in 1..=3 {
            stack.push(WindowId::new(number));
        }
        stack
    }

    fn top_down(stack: &Stack) -> Vec<u64> {
        stack.top_down().map(WindowId::get).collect()
    }

    #[test]
    fn new_windows_go_on_top() {
        let stack = three_windows();
        assert_eq!(top_down(&stack), [3, 2, 1]);
    }

    #[test]
    fn a_window_can_be_placed_directly_below_another() {
        let mut stack = three_windows();
        assert_eq!(stack.above(WindowId::new(1)), Some(WindowId::new(2)));
        assert_eq!(
            stack.above(WindowId::new(3)),
            None,
            "nothing is above the top"
        );
        assert!(stack.place_below(WindowId::new(3), WindowId::new(2)));
        assert_eq!(top_down(&stack), [2, 3, 1]);
        assert!(
            !stack.place_below(WindowId::new(3), WindowId::new(9)),
            "an unknown window is not stacked"
        );
        assert_eq!(top_down(&stack), [2, 3, 1]);
    }

    #[test]
    fn a_window_is_not_placed_below_one_of_another_layer() {
        let mut stack = three_windows();
        assert!(stack.set_layer(WindowId::new(2), StackLayer::Top));
        assert!(!stack.place_below(WindowId::new(1), WindowId::new(2)));
        assert_eq!(top_down(&stack), [2, 3, 1]);
    }

    #[test]
    fn raising_moves_a_window_to_the_top_and_keeps_the_rest_in_order() {
        let mut stack = three_windows();
        assert!(stack.raise(WindowId::new(1)));
        assert_eq!(top_down(&stack), [1, 3, 2]);
    }

    #[test]
    fn lowering_moves_a_window_to_the_bottom_and_keeps_the_rest_in_order() {
        let mut stack = three_windows();
        assert!(stack.lower(WindowId::new(3)));
        assert_eq!(top_down(&stack), [2, 1, 3]);
        assert!(
            !stack.lower(WindowId::new(9)),
            "an unknown window is not stacked"
        );
    }

    #[test]
    fn removing_a_window_leaves_the_others_in_order() {
        let mut stack = three_windows();
        assert!(stack.remove(WindowId::new(2)));
        assert!(
            !stack.remove(WindowId::new(2)),
            "it is gone after the first removal"
        );
        assert_eq!(top_down(&stack), [3, 1]);
    }

    #[test]
    fn raising_an_unknown_window_changes_nothing() {
        let mut stack = three_windows();
        assert!(!stack.raise(WindowId::new(9)));
        assert_eq!(top_down(&stack), [3, 2, 1]);
    }

    #[test]
    fn a_higher_layer_stays_above_whatever_is_raised() {
        let mut stack = three_windows();
        assert!(stack.set_layer(WindowId::new(1), StackLayer::Top));
        assert!(stack.raise(WindowId::new(2)));
        assert_eq!(top_down(&stack), [1, 2, 3]);
        stack.push(WindowId::new(4));
        assert_eq!(top_down(&stack), [1, 4, 2, 3], "new windows are normal");
    }

    #[test]
    fn lowering_stays_within_the_layer() {
        let mut stack = three_windows();
        assert!(stack.set_layer(WindowId::new(1), StackLayer::Desktop));
        assert!(stack.lower(WindowId::new(3)));
        assert_eq!(top_down(&stack), [2, 3, 1]);
        assert_eq!(stack.layer_of(WindowId::new(1)), Some(StackLayer::Desktop));
    }

    #[test]
    fn layers_read_by_name() {
        assert_eq!("above-dock".parse(), Ok(StackLayer::AboveDock));
        assert!("middle".parse::<StackLayer>().is_err());
        assert_eq!(StackLayer::Bottom.to_string(), "bottom");
    }

    #[test]
    #[should_panic(expected = "already stacked")]
    fn pushing_a_window_twice_is_a_bug() {
        let mut stack = three_windows();
        stack.push(WindowId::new(1));
    }
}
