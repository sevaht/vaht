//! Dialogs and the windows they belong to (transient windows), as Fluxbox
//! and fluxland treat them.
//!
//! A dialog names its parent: an xdg toplevel's `set_parent`, an X11
//! window's `WM_TRANSIENT_FOR`. A window and its dialogs, theirs too, are
//! one family: raised, lowered, minimized and restored together, the
//! dialogs always above their parent. A dialog opens centered over its
//! parent. A modal one (xdg-dialog's `set_modal`, X11's
//! `_NET_WM_STATE_MODAL`) keeps the focus while it is shown: focusing its
//! parent focuses it instead.

use super::window::{Area, Dialog, WindowId};
use super::{Window, WindowManager};

impl WindowManager {
    /// Makes `window` a dialog of `parent`, modal if `is_modal`, or with
    /// `None` a window of its own. A parent that is the window itself or
    /// one of its dialogs is ignored, so a family has no loops. A modal
    /// dialog of the focused window takes the focus.
    pub(crate) fn set_family(
        &mut self,
        window: WindowId,
        parent: Option<WindowId>,
        is_modal: bool,
    ) {
        let dialog = parent
            .filter(|parent| {
                *parent != window
                    && self.windows.get(*parent).is_some()
                    && !self.ancestors(*parent).any(|ancestor| ancestor == window)
            })
            .map(|parent| Dialog { parent, is_modal });
        let Some(target) = self.window_mut(window) else {
            return;
        };
        if target.dialog() == dialog {
            return;
        }
        target.set_dialog(dialog);
        if let Some(Dialog {
            parent,
            is_modal: true,
        }) = dialog
            && self.focused() == Some(parent)
        {
            self.focus(Some(parent));
        }
    }

    /// Returns the windows `window` is a dialog of, nearest first.
    pub(super) fn ancestors(&self, window: WindowId) -> impl Iterator<Item = WindowId> + '_ {
        let mut next = self.windows.get(window).and_then(Window::parent);
        // A family has no loops, but a bound costs nothing.
        (0..self.windows.len()).map_while(move |_| {
            let current = next?;
            next = self.windows.get(current).and_then(Window::parent);
            Some(current)
        })
    }

    /// Returns the window to focus for `window`: its innermost shown modal
    /// dialog, if it has one, else itself.
    pub(super) fn focus_for(&self, window: WindowId) -> WindowId {
        let current = self.workspaces.current();
        let mut focus = window;
        for _ in 0..self.windows.len() {
            let dialog = self
                .windows
                .top_down()
                .find(|dialog| {
                    dialog.parent() == Some(focus)
                        && dialog.is_modal()
                        && dialog.is_shown_on(current)
                })
                .map(Window::id);
            match dialog {
                Some(dialog) => focus = dialog,
                None => break,
            }
        }
        focus
    }

    /// Returns `window` and its dialogs, theirs too, each parent before its
    /// dialogs and dialogs of one parent bottom-up: the order to raise
    /// them in so the family keeps its order, above all else.
    pub(super) fn family_bottom_up(&self, window: WindowId) -> Vec<WindowId> {
        let mut family = vec![window];
        let mut next = 0;
        while let Some(member) = family.get(next).copied() {
            let mut dialogs: Vec<WindowId> = self
                .windows
                .top_down()
                .filter(|dialog| dialog.parent() == Some(member))
                .map(Window::id)
                .filter(|dialog| !family.contains(dialog))
                .collect();
            dialogs.reverse();
            family.extend(dialogs);
            next += 1;
        }
        family
    }

    /// Makes the dialogs of `gone`, which closed, windows of their own.
    pub(super) fn orphan_dialogs_of(&mut self, gone: WindowId) {
        for window in self.windows.iter_mut() {
            if window.parent() == Some(gone) {
                window.set_dialog(None);
            }
        }
    }

    /// Returns where a dialog `size` large opens: centered over its
    /// parent's frame, kept within the usable area of the parent's output;
    /// none for a window that is no dialog or whose parent is not placed.
    pub(super) fn dialog_area(
        &self,
        window: WindowId,
        size: smithay::utils::Size<i32, smithay::utils::Logical>,
    ) -> Option<Area> {
        let parent = self.windows.get(self.windows.get(window)?.parent()?)?;
        if !parent.is_placed() {
            return None;
        }
        let over = parent.outer_area(self.metrics);
        let usable = self
            .outputs
            .at(over.loc + over.size.downscale(2).to_point())
            .usable;
        let centered = |start: i32, length: i32, room: i32| start + (length - room) / 2;
        let clamp =
            |at: i32, length: i32, start: i32, room: i32| at.min(start + room - length).max(start);
        let x = centered(over.loc.x, over.size.w, size.w);
        let y = centered(over.loc.y, over.size.h, size.h);
        Some(Area::new(
            (
                clamp(x, size.w, usable.loc.x, usable.size.w),
                clamp(y, size.h, usable.loc.y, usable.size.h),
            )
                .into(),
            size,
        ))
    }
}
