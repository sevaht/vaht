//! Picking one of a list a program sends (`vaht pick`): a dmenu-style
//! chooser drawn as one of vaht's own menus, in the user's style, at the
//! pointer. The screen-share portal (xdg-desktop-portal-wlr) uses it to ask
//! which output or window to share (`data/xdg-desktop-portal-wlr-vaht`).
//!
//! The IPC reply waits for the user: choosing an item answers with it,
//! closing the menu any other way answers with none. One pick shows at a
//! time; a new one answers the one before with none.

use vaht_ipc::Reply;

use crate::Compositor;
use crate::ipc::{Answer, IpcServer};
use crate::menu::{MenuChoice, MenuItem, MenuModel};

/// A pick shown, waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pick {
    /// The IPC connection to answer.
    connection: u64,
    /// The items, as sent.
    items: Vec<String>,
}

impl Compositor {
    /// Shows `items` as a menu titled `title` for IPC connection
    /// `connection`, answered once the user chooses; at once with none if
    /// there is nothing to pick or the session is locked.
    pub(crate) fn start_pick(
        &mut self,
        connection: u64,
        title: Option<String>,
        items: Vec<String>,
    ) -> Answer {
        if items.is_empty() || self.session_lock.is_locked() {
            return Answer::Now(Reply::Picked { choice: None });
        }
        // One pick at a time: the one before goes unanswered by the user.
        self.answer_pick(None);
        tracing::info!(count = items.len(), "asking the user to pick");
        let model = MenuModel {
            title,
            items: items
                .iter()
                .enumerate()
                .map(|(index, label)| MenuItem::Choice {
                    label: label.clone(),
                    choice: MenuChoice::AnswerPick(index),
                    mark: None,
                    icon: None,
                })
                .collect(),
        };
        self.open_menu_at_pointer(model);
        self.pick = Some(Pick { connection, items });
        Answer::Later
    }

    /// Answers the pick waiting, if any, with item `index`, or none.
    pub(crate) fn answer_pick(&mut self, index: Option<usize>) {
        let Some(pick) = self.pick.take() else {
            return;
        };
        let choice = index.and_then(|index| pick.items.get(index).cloned());
        tracing::debug!(?choice, "pick answered");
        IpcServer::answer_later(self, pick.connection, &Reply::Picked { choice });
    }

    /// Follows the pick waiting: its menu closed without a choice answers
    /// it with none, and its program gone closes its menu.
    pub(crate) fn follow_pick(&mut self) {
        let Some(pick) = &self.pick else {
            return;
        };
        let is_asking = self
            .ipc
            .as_ref()
            .is_some_and(|server| server.is_connected(pick.connection));
        if !is_asking {
            self.pick = None;
            self.menus.close();
            self.schedule_render();
        } else if !self.menus.is_open() {
            self.answer_pick(None);
        }
    }
}
