//! Following key chains and key modes as chords are pressed.
//!
//! A chain's chords are pressed in turn ([`crate::config::Bindings`]):
//! after its first, the chords pressed so far wait for the next, for up to
//! [`KeyChain::TIMEOUT`] between presses, as Fluxbox waits. Escape gives
//! up on a chain, and so does a chord it does not go on with, which then
//! reaches the focused window as any unbound key does. Modifier keys
//! pressed for the next chord neither end nor go on with one.
//!
//! A key mode (`key-mode <name>`) makes its own bindings the only ones,
//! until its end chord, unbound in the mode, goes back to `[keys]`.
//!
//! It is plain values fed chords and the time of each press, so tests
//! follow it alone (GENERAL T7).

use std::time::{Duration, Instant};

use crate::config::{Bindings, Command, KeyChord, KeyLookup, KeyModeSwitch};

/// What a chord pressed does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChordOutcome {
    /// It ends a binding, which runs this.
    Run(Command),
    /// It is vaht's, running nothing yet: a chain goes on, or ends.
    Taken,
    /// It is the focused window's.
    Pass,
}

/// The chords pressed toward a chain, and the key mode.
#[derive(Debug, Clone, Default)]
pub(crate) struct KeyChain {
    /// The chords of the chain pressed so far, none outside one.
    pressed: Vec<KeyChord>,
    /// When the last of them was pressed.
    last_press: Option<Instant>,
    /// The key mode, if not `[keys]`'s.
    mode: Option<KeyModeSwitch>,
}

impl KeyChain {
    /// How long a chain waits for its next chord.
    pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);

    /// Returns what `chord`, pressed at `now`, does under `bindings`.
    pub(crate) fn press(
        &mut self,
        chord: KeyChord,
        now: Instant,
        bindings: &Bindings,
    ) -> ChordOutcome {
        let is_waiting = !self.pressed.is_empty();
        if is_waiting && chord.is_modifier() {
            return ChordOutcome::Pass;
        }
        let has_waited_too_long = self
            .last_press
            .is_some_and(|last| now.saturating_duration_since(last) > Self::TIMEOUT);
        if is_waiting && has_waited_too_long {
            tracing::debug!("key chain timed out");
            self.give_up();
        } else if is_waiting && chord == KeyChord::escape() {
            tracing::debug!("key chain given up");
            self.give_up();
            return ChordOutcome::Taken;
        }
        self.pressed.push(chord);
        match bindings.key(self.mode(), &self.pressed) {
            KeyLookup::Bound(command) => {
                self.give_up();
                ChordOutcome::Run(command.clone())
            }
            KeyLookup::Chain => {
                self.last_press = Some(now);
                ChordOutcome::Taken
            }
            KeyLookup::Unbound => {
                let was_in_chain = self.pressed.len() > 1;
                self.give_up();
                if !was_in_chain && self.mode.as_ref().is_some_and(|mode| mode.end == chord) {
                    tracing::info!("key mode ended");
                    self.mode = None;
                    return ChordOutcome::Taken;
                }
                ChordOutcome::Pass
            }
        }
    }

    /// Switches to the key mode `switch`, or back to `[keys]` if it is the
    /// default or `bindings` have no such mode; any chain is given up.
    pub(crate) fn switch_mode(&mut self, switch: &KeyModeSwitch, bindings: &Bindings) {
        self.give_up();
        if switch.is_default() || !bindings.has_key_mode(&switch.name) {
            self.mode = None;
        } else {
            self.mode = Some(switch.clone());
        }
        tracing::info!(mode = %switch.name, "key mode switched");
    }

    /// Follows `bindings` read again: any chain is given up, and a mode
    /// they no longer have goes back to `[keys]`.
    pub(crate) fn bindings_changed(&mut self, bindings: &Bindings) {
        self.give_up();
        if self
            .mode
            .as_ref()
            .is_some_and(|mode| !bindings.has_key_mode(&mode.name))
        {
            self.mode = None;
        }
    }

    /// Returns the key mode's name, if not `[keys]`'s.
    pub(crate) fn mode(&self) -> Option<&str> {
        self.mode.as_ref().map(|mode| mode.name.as_str())
    }

    /// Forgets the chords pressed toward a chain.
    fn give_up(&mut self) {
        self.pressed.clear();
        self.last_press = None;
    }
}

#[cfg(test)]
mod tests {
    use smithay::input::keyboard::Keysym;

    use super::*;
    use crate::config::Action;
    use crate::config::Modifiers;

    /// The bindings of `text`.
    fn bindings(text: &str) -> Bindings {
        toml::from_str(text).expect("valid bindings")
    }

    /// `key` pressed alone.
    fn plain(key: Keysym) -> KeyChord {
        KeyChord::new(Modifiers::default(), key)
    }

    /// The command `text`.
    fn run(text: &str) -> ChordOutcome {
        ChordOutcome::Run(Command::Run(Action::parse(text).expect("valid")))
    }

    /// The switch `text` writes after `key-mode`.
    fn switch(text: &str) -> KeyModeSwitch {
        match Action::parse(&format!("key-mode {text}")) {
            Ok(Action::KeyMode(switch)) => switch,
            other => unreachable!("not a key mode switch: {other:?}"),
        }
    }

    const CHAIN: &str = "[keys]\n\"x t\" = \"exec kitty\"\n\"r\" = \"reload\"\n";

    #[test]
    fn a_chain_runs_on_its_last_chord() {
        let bindings = bindings(CHAIN);
        let mut chain = KeyChain::default();
        let now = Instant::now();
        assert_eq!(
            chain.press(plain(Keysym::x), now, &bindings),
            ChordOutcome::Taken
        );
        assert_eq!(
            chain.press(plain(Keysym::Shift_L), now, &bindings),
            ChordOutcome::Pass,
            "a modifier pressed for the next chord"
        );
        assert_eq!(
            chain.press(plain(Keysym::t), now, &bindings),
            run("exec kitty")
        );
        assert_eq!(
            chain.press(plain(Keysym::t), now, &bindings),
            ChordOutcome::Pass
        );
    }

    #[test]
    fn a_chain_is_given_up_by_escape_another_chord_or_waiting() {
        let bindings = bindings(CHAIN);
        let mut chain = KeyChain::default();
        let now = Instant::now();
        chain.press(plain(Keysym::x), now, &bindings);
        assert_eq!(
            chain.press(KeyChord::escape(), now, &bindings),
            ChordOutcome::Taken
        );
        assert_eq!(
            chain.press(plain(Keysym::t), now, &bindings),
            ChordOutcome::Pass
        );

        chain.press(plain(Keysym::x), now, &bindings);
        assert_eq!(
            chain.press(plain(Keysym::r), now, &bindings),
            ChordOutcome::Pass,
            "the window gets the chord that broke the chain"
        );
        assert_eq!(chain.press(plain(Keysym::r), now, &bindings), run("reload"));

        chain.press(plain(Keysym::x), now, &bindings);
        let later = now + KeyChain::TIMEOUT + Duration::from_millis(1);
        assert_eq!(
            chain.press(plain(Keysym::r), later, &bindings),
            run("reload"),
            "a fresh press after the chain timed out"
        );
    }

    #[test]
    fn a_key_mode_binds_only_its_keys_until_its_end_chord() {
        let bindings =
            bindings("[keys]\n\"r\" = \"reload\"\n[key_modes.moving]\n\"Left\" = \"move -10 0\"\n");
        let mut chain = KeyChain::default();
        let now = Instant::now();
        chain.switch_mode(&switch("moving"), &bindings);
        assert_eq!(chain.mode(), Some("moving"));
        assert_eq!(
            chain.press(plain(Keysym::Left), now, &bindings),
            run("move -10 0")
        );
        assert_eq!(
            chain.press(plain(Keysym::r), now, &bindings),
            ChordOutcome::Pass,
            "[keys] is not bound in a mode"
        );
        assert_eq!(
            chain.press(KeyChord::escape(), now, &bindings),
            ChordOutcome::Taken
        );
        assert_eq!(chain.mode(), None);
        assert_eq!(chain.press(plain(Keysym::r), now, &bindings), run("reload"));

        chain.switch_mode(&switch("moving q"), &bindings);
        assert_eq!(
            chain.press(KeyChord::escape(), now, &bindings),
            ChordOutcome::Pass
        );
        assert_eq!(
            chain.press(plain(Keysym::q), now, &bindings),
            ChordOutcome::Taken
        );
        assert_eq!(chain.mode(), None);
    }

    #[test]
    fn an_unknown_mode_or_one_read_away_is_keys() {
        let moving = bindings("[key_modes.moving]\n\"Left\" = \"move -10 0\"\n");
        let mut chain = KeyChain::default();
        chain.switch_mode(&switch("resizing"), &moving);
        assert_eq!(chain.mode(), None);
        chain.switch_mode(&switch("moving"), &moving);
        chain.bindings_changed(&bindings(""));
        assert_eq!(chain.mode(), None);
        chain.switch_mode(&switch("moving"), &moving);
        chain.switch_mode(&switch("default"), &moving);
        assert_eq!(chain.mode(), None);
    }
}
