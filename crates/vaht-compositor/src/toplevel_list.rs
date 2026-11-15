//! The windows listed for programs outside vaht
//! (`ext_foreign_toplevel_list_v1`): docks, window switchers and
//! screen-sharing pickers.
//!
//! [`ToplevelList`] follows the window manager's windows, and tells an
//! [`Announcer`] of each one opened, renamed (title or app id) or closed;
//! it is plain values, so tests follow it with an announcer of their own.
//! [`ForeignToplevels`] announces through Smithay.

use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState};

use crate::Compositor;

/// Tells programs of the windows.
pub(crate) trait Announcer {
    /// What stands for an announced window.
    type Handle;

    /// Announces a window titled `title`, of the program `app_id`.
    fn announce(&mut self, title: &str, app_id: &str) -> Self::Handle;

    /// Announces that the window of `handle` is now titled `title`, of the
    /// program `app_id`.
    fn rename(&mut self, handle: &Self::Handle, title: &str, app_id: &str);

    /// Announces that the window of `handle` closed.
    fn withdraw(&mut self, handle: Self::Handle);
}

/// A window as the list knows it.
#[derive(Debug)]
struct Listed<Key, Handle> {
    /// The window.
    key: Key,
    /// Its title, as last announced.
    title: String,
    /// Its program, as last announced.
    app_id: String,
    /// What stands for it.
    handle: Handle,
}

/// The windows announced, in the order they opened.
#[derive(Debug)]
pub(crate) struct ToplevelList<Key, Handle> {
    /// Each window announced.
    listed: Vec<Listed<Key, Handle>>,
}

impl<Key, Handle> Default for ToplevelList<Key, Handle> {
    fn default() -> Self {
        Self { listed: Vec::new() }
    }
}

impl<Key: Ord + Copy, Handle> ToplevelList<Key, Handle> {
    /// Follows the windows now open, `windows` (each with its title and
    /// program): those new are announced, in the order of their keys
    /// (which is the order they opened), those renamed announced again,
    /// and those gone withdrawn.
    pub(crate) fn follow<'window, A: Announcer<Handle = Handle>>(
        &mut self,
        windows: impl IntoIterator<Item = (Key, &'window str, &'window str)>,
        announcer: &mut A,
    ) {
        let mut windows: Vec<(Key, &str, &str)> = windows.into_iter().collect();
        windows.sort_by_key(|(key, ..)| *key);
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.listed)
            .into_iter()
            .partition(|listed| windows.iter().any(|(key, ..)| *key == listed.key));
        for listed in gone {
            announcer.withdraw(listed.handle);
        }
        self.listed = kept;
        for (key, title, app_id) in windows {
            match self.listed.iter_mut().find(|listed| listed.key == key) {
                Some(listed) => {
                    if listed.title != title || listed.app_id != app_id {
                        announcer.rename(&listed.handle, title, app_id);
                        title.clone_into(&mut listed.title);
                        app_id.clone_into(&mut listed.app_id);
                    }
                }
                None => self.listed.push(Listed {
                    key,
                    title: title.to_owned(),
                    app_id: app_id.to_owned(),
                    handle: announcer.announce(title, app_id),
                }),
            }
        }
    }
}

impl<Key: Copy, Handle> ToplevelList<Key, Handle> {
    /// Returns the window whose handle `is_it`, if it is still listed.
    pub(crate) fn window_where(&self, is_it: impl Fn(&Handle) -> bool) -> Option<Key> {
        self.listed
            .iter()
            .find(|listed| is_it(&listed.handle))
            .map(|listed| listed.key)
    }
}

/// Announces windows through Smithay's `ext_foreign_toplevel_list_v1`.
#[derive(Debug)]
pub(crate) struct ForeignToplevels<'state> {
    /// The protocol's state.
    pub(crate) state: &'state mut ForeignToplevelListState,
}

impl Announcer for ForeignToplevels<'_> {
    type Handle = ForeignToplevelHandle;

    fn announce(&mut self, title: &str, app_id: &str) -> ForeignToplevelHandle {
        self.state.new_toplevel::<Compositor>(title, app_id)
    }

    fn rename(&mut self, handle: &ForeignToplevelHandle, title: &str, app_id: &str) {
        handle.send_title(title);
        handle.send_app_id(app_id);
        handle.send_done();
    }

    fn withdraw(&mut self, handle: ForeignToplevelHandle) {
        self.state.remove_toplevel(&handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what it was told, as lines.
    #[derive(Debug, Default)]
    struct Recorder {
        /// What it was told, in order.
        told: Vec<String>,
        /// Handles given so far.
        given: u32,
    }

    impl Announcer for Recorder {
        type Handle = u32;

        fn announce(&mut self, title: &str, app_id: &str) -> u32 {
            self.given += 1;
            self.told
                .push(format!("open {} {title} {app_id}", self.given));
            self.given
        }

        fn rename(&mut self, handle: &u32, title: &str, app_id: &str) {
            self.told.push(format!("rename {handle} {title} {app_id}"));
        }

        fn withdraw(&mut self, handle: u32) {
            self.told.push(format!("close {handle}"));
        }
    }

    #[test]
    fn windows_are_announced_once_renamed_on_change_and_withdrawn() {
        let mut list = ToplevelList::default();
        let mut recorder = Recorder::default();
        list.follow([(2, "Shell", "term"), (1, "Editor", "edit")], &mut recorder);
        list.follow([(1, "Editor", "edit"), (2, "Shell", "term")], &mut recorder);
        list.follow([(1, "Editor — a.txt", "edit")], &mut recorder);
        assert_eq!(
            recorder.told,
            [
                "open 1 Editor edit",
                "open 2 Shell term",
                "close 2",
                "rename 1 Editor — a.txt edit",
            ]
        );
    }
}
