//! Workspaces: the desktops windows are spread over (01 §5.3).
//!
//! Workspaces are global, shared by every output, as in Fluxbox. Which
//! workspace a window is on is the window's own field; [`Workspaces`] holds
//! only their names and which one is shown, so membership is never recorded
//! twice. Callers ask [`Workspaces::current`], which will take an output
//! should workspaces ever become per output.

use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;

/// One workspace, by position: the first is workspace 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct Workspace(usize);

impl Workspace {
    /// Returns the workspace with 1-based `number`.
    pub(crate) const fn numbered(number: NonZeroUsize) -> Self {
        Self(number.get() - 1)
    }
}

/// Writes the 1-based number, as users count workspaces.
impl fmt::Display for Workspace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0 + 1)
    }
}

/// Which workspace an action means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WorkspaceTarget {
    /// The one with this 1-based number.
    Numbered(NonZeroUsize),
    /// The one after the current one, wrapping around.
    Next,
    /// The one before the current one, wrapping around.
    Previous,
    /// The one shown before the current one.
    Last,
}

impl FromStr for WorkspaceTarget {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "next" => Ok(Self::Next),
            "previous" => Ok(Self::Previous),
            "last" => Ok(Self::Last),
            number => number.parse().map(Self::Numbered).map_err(|_not_a_number| {
                format!("a workspace is a number from 1, next, previous or last, not `{text}`")
            }),
        }
    }
}

impl fmt::Display for WorkspaceTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numbered(number) => write!(formatter, "{number}"),
            Self::Next => formatter.write_str("next"),
            Self::Previous => formatter.write_str("previous"),
            Self::Last => formatter.write_str("last"),
        }
    }
}

/// The workspaces, and which one is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspaces {
    /// Each workspace's name; there is always at least one.
    names: Vec<String>,
    /// The one shown.
    current: Workspace,
    /// The one shown before it.
    previous: Workspace,
}

impl Workspaces {
    /// How many workspaces there are by default.
    pub(crate) const DEFAULT_COUNT: NonZeroUsize = NonZeroUsize::new(4).expect("four is not zero");

    /// Creates `count` workspaces with default names, showing the first.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            names: (1..=count.get()).map(Self::default_name).collect(),
            current: Workspace::default(),
            previous: Workspace::default(),
        }
    }

    /// Returns the workspace shown.
    pub(crate) const fn current(&self) -> Workspace {
        self.current
    }

    /// Returns how many workspaces there are.
    pub(crate) const fn count(&self) -> usize {
        self.names.len()
    }

    /// Returns the last workspace.
    pub(crate) fn last(&self) -> Workspace {
        Workspace(self.count() - 1)
    }

    /// Returns the workspace `target` means, if it exists.
    pub(crate) fn resolve(&self, target: WorkspaceTarget) -> Option<Workspace> {
        let count = self.count();
        let current = self.current.0;
        let workspace = match target {
            WorkspaceTarget::Numbered(number) => Workspace::numbered(number),
            WorkspaceTarget::Next => Workspace((current + 1) % count),
            WorkspaceTarget::Previous => Workspace((current + count - 1) % count),
            WorkspaceTarget::Last => self.previous,
        };
        (workspace.0 < count).then_some(workspace)
    }

    /// Shows `workspace`; returns whether that changed anything.
    pub(crate) fn show(&mut self, workspace: Workspace) -> bool {
        let changes = workspace != self.current && workspace.0 < self.count();
        if changes {
            self.previous = self.current;
            self.current = workspace;
        }
        changes
    }

    /// Adds a workspace at the end, and returns it.
    pub(crate) fn add(&mut self) -> Workspace {
        self.names.push(Self::default_name(self.count() + 1));
        Workspace(self.count() - 1)
    }

    /// Removes the last workspace, unless it is the only one, and returns
    /// it; the workspace before it is shown if it was.
    pub(crate) fn remove_last(&mut self) -> Option<Workspace> {
        if self.count() == 1 {
            return None;
        }
        self.names.pop();
        let removed = Workspace(self.count());
        let last_left = Workspace(self.count() - 1);
        if self.current == removed {
            self.current = last_left;
        }
        if self.previous == removed {
            self.previous = last_left;
        }
        Some(removed)
    }

    /// Returns the name a new workspace with 1-based `number` gets.
    fn default_name(number: usize) -> String {
        format!("Workspace {number}")
    }
}

impl Default for Workspaces {
    fn default() -> Self {
        Self::new(Self::DEFAULT_COUNT)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Four workspaces, showing the one with 1-based `number`.
    fn showing(number: usize) -> Workspaces {
        let mut workspaces = Workspaces::default();
        workspaces.show(Workspace(number - 1));
        workspaces
    }

    #[rstest]
    #[case::next(1, "next", Some(2))]
    #[case::next_wraps(4, "next", Some(1))]
    #[case::previous_wraps(1, "previous", Some(4))]
    #[case::numbered(2, "3", Some(3))]
    #[case::beyond_the_last(1, "5", None)]
    fn targets_resolve_from_the_current_workspace(
        #[case] current: usize,
        #[case] target: &str,
        #[case] expected: Option<usize>,
    ) {
        let target: WorkspaceTarget = target.parse().expect("a valid target");
        assert_eq!(
            showing(current).resolve(target),
            expected.map(|number| Workspace(number - 1))
        );
    }

    #[test]
    fn last_is_the_one_shown_before() {
        let mut workspaces = showing(3);
        workspaces.show(Workspace(0));
        assert_eq!(
            workspaces.resolve(WorkspaceTarget::Last),
            Some(Workspace(2))
        );
    }

    #[test]
    fn removing_the_shown_workspace_shows_the_one_before() {
        let mut workspaces = showing(4);
        assert_eq!(workspaces.remove_last(), Some(Workspace(3)));
        assert_eq!(workspaces.current(), Workspace(2));
        let mut single = Workspaces::new(NonZeroUsize::MIN);
        assert_eq!(single.remove_last(), None, "the only workspace stays");
    }

    #[rstest]
    #[case::zero("0")]
    #[case::word("fifth")]
    fn mistakes_are_explained(#[case] text: &str) {
        let error = text.parse::<WorkspaceTarget>().expect_err(text);
        assert!(error.contains("a workspace is a number"), "{error}");
    }
}
