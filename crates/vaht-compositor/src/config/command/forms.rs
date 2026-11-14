//! The structured command forms of 02 §4 beyond `repeat`: `toggle`, `if`,
//! `for_each` and `delay_ms` (Fluxbox's `ToggleCmd`, `If`, `ForEach`
//! and `Delay`).
//!
//! Each is plain data read from a table; the compositor carries them out.
//! A toggle remembers which of its commands runs next, and a delay is
//! known by its own identity, so running it again before it fires puts it
//! off rather than running it twice.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{Command, WindowMatcher};
use crate::window_manager::{Window, Workspace};

/// Commands run in turn, one each time (`{ toggle = [a, b] }`).
#[derive(Debug, Clone)]
pub(crate) struct Toggle {
    /// The commands, at least one.
    pub(crate) commands: Vec<Command>,
    /// How many times it has run; shared by its copies.
    turns: Arc<AtomicUsize>,
}

impl Toggle {
    /// Commands run in turn, the first first; none is no toggle.
    pub(crate) fn new(commands: Vec<Command>) -> Option<Self> {
        (!commands.is_empty()).then(|| Self {
            commands,
            turns: Arc::default(),
        })
    }

    /// Returns the command whose turn it is, and passes the turn on.
    pub(crate) fn next(&self) -> &Command {
        let turn = self.turns.fetch_add(1, Ordering::Relaxed);
        &self.commands[turn % self.commands.len()]
    }
}

impl PartialEq for Toggle {
    fn eq(&self, other: &Self) -> bool {
        self.commands == other.commands
    }
}

impl Eq for Toggle {}

/// A command run a while later, put off again if it runs again first
/// (`{ delay_ms = 300, do = cmd }`).
#[derive(Debug, Clone)]
pub(crate) struct Delay {
    /// How long it waits.
    pub(crate) wait: Duration,
    /// What runs.
    pub(crate) command: Box<Command>,
    /// Who it is, shared by its copies, so a pending run is found again.
    identity: Arc<()>,
}

impl Delay {
    /// `command`, run `wait` later.
    pub(crate) fn new(wait: Duration, command: Command) -> Self {
        Self {
            wait,
            command: Box::new(command),
            identity: Arc::new(()),
        }
    }

    /// Returns whether `other` is this delay (or a copy of it).
    pub(crate) fn is(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl PartialEq for Delay {
    fn eq(&self, other: &Self) -> bool {
        self.wait == other.wait && self.command == other.command
    }
}

impl Eq for Delay {}

/// What `if` asks of the window a command is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Condition {
    /// The window matches.
    Matches(WindowMatcher),
    /// Every condition holds.
    All(Vec<Condition>),
    /// Some condition holds.
    Any(Vec<Condition>),
    /// The condition does not hold.
    Not(Box<Condition>),
}

impl Condition {
    /// Returns whether the condition holds of `window` (none never
    /// matches), with `current` the workspace shown.
    pub(crate) fn holds(&self, window: Option<&Window>, current: Workspace) -> bool {
        match self {
            Self::Matches(matcher) => window.is_some_and(|window| matcher.matches(window, current)),
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.holds(window, current)),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.holds(window, current)),
            Self::Not(condition) => !condition.holds(window, current),
        }
    }

    /// Reads a condition: `{ all = [..] }`, `{ any = [..] }`,
    /// `{ not = .. }`, or a window matcher.
    pub(crate) fn from_value(value: &toml::Value) -> Result<Self, String> {
        let table = value
            .as_table()
            .ok_or("a condition is a table: a window matcher, or all, any or not")?;
        let list = |value: &toml::Value| -> Result<Vec<Self>, String> {
            value
                .as_array()
                .ok_or("`all` and `any` take an array of conditions")?
                .iter()
                .map(Self::from_value)
                .collect()
        };
        match (table.len(), table.iter().next()) {
            (1, Some((key, inner))) if key == "all" => list(inner).map(Self::All),
            (1, Some((key, inner))) if key == "any" => list(inner).map(Self::Any),
            (1, Some((key, inner))) if key == "not" => {
                Self::from_value(inner).map(|condition| Self::Not(Box::new(condition)))
            }
            _ => value
                .clone()
                .try_into::<WindowMatcher>()
                .map(Self::Matches)
                .map_err(|error| format!("a condition: {}", error.message())),
        }
    }
}
