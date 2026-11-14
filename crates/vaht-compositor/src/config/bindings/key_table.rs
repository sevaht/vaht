//! Key bindings: chains of chords and what they run (02 §6).
//!
//! A binding's key is one chord (`Super+T`) or a chain of them separated
//! by spaces (`Super+A Super+B`, Fluxbox's chained bindings): the chain
//! runs its command once every chord of it is pressed in turn. A chain's
//! beginning cannot be bound itself, as nothing would tell the two apart.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::de::{self, Deserializer};

use super::super::command::Command;
use super::super::unique_map;
use super::KeyChord;

/// Chords pressed one after another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct KeyChain(Vec<KeyChord>);

impl KeyChain {
    /// Parses `Super+A Super+B`.
    fn parse(text: &str) -> Result<Self, String> {
        let chords = text
            .split_whitespace()
            .map(KeyChord::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if chords.is_empty() {
            return Err("a key binding needs a key".to_owned());
        }
        Ok(Self(chords))
    }
}

impl fmt::Display for KeyChain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut chords = self.0.iter();
        if let Some(first) = chords.next() {
            first.fmt(formatter)?;
        }
        for chord in chords {
            write!(formatter, " {chord}")?;
        }
        Ok(())
    }
}

impl<'de> serde::Deserialize<'de> for KeyChain {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

/// What the chords pressed so far are bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyLookup<'table> {
    /// A whole binding: this runs.
    Bound(&'table Command),
    /// The beginning of a chain: more chords are to come.
    Chain,
    /// Nothing.
    Unbound,
}

/// The key bindings of `[keys]` or of a key mode.
#[derive(Debug, Clone, Default)]
pub(crate) struct KeyTable {
    /// Each binding.
    bindings: HashMap<KeyChain, Command>,
    /// Every beginning of a chain bound, shorter than the chain.
    beginnings: HashSet<Vec<KeyChord>>,
}

impl KeyTable {
    /// Returns what `pressed`, chords pressed in turn, is bound to.
    pub(crate) fn lookup(&self, pressed: &[KeyChord]) -> KeyLookup<'_> {
        if let Some(command) = self.bindings.get(&KeyChain(pressed.to_vec())) {
            KeyLookup::Bound(command)
        } else if self.beginnings.contains(pressed) {
            KeyLookup::Chain
        } else {
            KeyLookup::Unbound
        }
    }

    /// Returns every command bound.
    pub(crate) fn commands(&self) -> impl Iterator<Item = &Command> {
        self.bindings.values()
    }

    /// Builds the table of `bindings`, refusing a chain whose beginning is
    /// bound too, or a chain that repeats.
    fn new(bindings: HashMap<KeyChain, Command>) -> Result<Self, String> {
        let mut beginnings = HashSet::new();
        for (chain, command) in &bindings {
            if chain.0.len() > 1 && command.repeats() {
                return Err(format!("`{chain}`: a chain of keys cannot repeat"));
            }
            for length in 1..chain.0.len() {
                let beginning = &chain.0[..length];
                if bindings.contains_key(&KeyChain(beginning.to_vec())) {
                    return Err(format!(
                        "`{}` is bound, so it cannot begin `{chain}`",
                        KeyChain(beginning.to_vec())
                    ));
                }
                beginnings.insert(beginning.to_vec());
            }
        }
        Ok(Self {
            bindings,
            beginnings,
        })
    }
}

impl<'de> serde::Deserialize<'de> for KeyTable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bindings = unique_map::deserialize(deserializer)?;
        Self::new(bindings).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use smithay::input::keyboard::Keysym;

    use super::super::Modifiers;
    use super::*;
    use crate::config::command::Action;

    /// Parses a table.
    fn table(text: &str) -> Result<KeyTable, String> {
        toml::from_str(text).map_err(|error| error.message().to_owned())
    }

    /// `key` pressed with Super held.
    fn with_super(key: Keysym) -> KeyChord {
        KeyChord::new(Modifiers::named("Super").expect("a modifier"), key)
    }

    #[test]
    fn a_chain_runs_once_every_chord_is_pressed() {
        let parsed =
            table("\"Super+X Super+T\" = \"exec kitty\"\n\"Super+R\" = \"reload\"").expect("valid");
        assert_eq!(parsed.lookup(&[with_super(Keysym::x)]), KeyLookup::Chain);
        assert_eq!(
            parsed.lookup(&[with_super(Keysym::x), with_super(Keysym::t)]),
            KeyLookup::Bound(&Command::Run(Action::Exec("kitty".to_owned())))
        );
        assert_eq!(
            parsed.lookup(&[with_super(Keysym::x), with_super(Keysym::r)]),
            KeyLookup::Unbound,
            "a binding of its own is not part of the chain"
        );
        assert_eq!(
            parsed.lookup(&[with_super(Keysym::r)]),
            KeyLookup::Bound(&Command::Run(Action::Reload))
        );
        assert_eq!(
            parsed.lookup(&[KeyChord::new(Modifiers::default(), Keysym::x)]),
            KeyLookup::Unbound
        );
    }

    #[test]
    fn a_chain_writes_what_it_reads() {
        let chain = KeyChain::parse("Super+x  t").expect("valid");
        assert_eq!(chain.to_string(), "Super+x t");
    }

    #[test]
    fn a_bound_beginning_or_repeating_chain_is_refused() {
        let error =
            table("\"Super+X\" = \"reload\"\n\"Super+X T\" = \"exit\"").expect_err("ambiguous");
        assert!(error.contains("cannot begin"), "{error}");
        let error = table("\"Super+X T\" = { repeat = \"exit\" }").expect_err("a repeating chain");
        assert!(error.contains("cannot repeat"), "{error}");
        assert!(table("\"\" = \"exit\"").is_err());
    }
}
