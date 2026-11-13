//! The stacking order of windows: the single record of what is above what.

use crate::window_manager::WindowId;

/// Windows from bottom to top; the last is drawn above all others.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stack {
    /// Window ids, bottom first.
    bottom_to_top: Vec<WindowId>,
}

impl Stack {
    /// Returns the windows from top to bottom, the order hit-testing uses.
    pub(crate) fn top_down(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.bottom_to_top.iter().rev().copied()
    }

    /// Places `id` above every other window.
    ///
    /// # Panics
    ///
    /// Panics if `id` is already in the stack: an id enters once, when its
    /// window is created.
    pub(crate) fn push(&mut self, id: WindowId) {
        assert!(
            !self.bottom_to_top.contains(&id),
            "window {id} is already stacked"
        );
        self.bottom_to_top.push(id);
    }

    /// Takes `id` out of the stack; returns whether it was there.
    pub(crate) fn remove(&mut self, id: WindowId) -> bool {
        let before = self.bottom_to_top.len();
        self.bottom_to_top.retain(|stacked| *stacked != id);
        self.bottom_to_top.len() != before
    }

    /// Moves `id` below every other window; returns whether it was stacked.
    pub(crate) fn lower(&mut self, id: WindowId) -> bool {
        let lowered = self.remove(id);
        if lowered {
            self.bottom_to_top.insert(0, id);
        }
        lowered
    }

    /// Moves `id` above every other window; returns whether it was stacked.
    pub(crate) fn raise(&mut self, id: WindowId) -> bool {
        let raised = self.remove(id);
        if raised {
            self.bottom_to_top.push(id);
        }
        raised
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
    #[should_panic(expected = "already stacked")]
    fn pushing_a_window_twice_is_a_bug() {
        let mut stack = three_windows();
        stack.push(WindowId::new(1));
    }
}
