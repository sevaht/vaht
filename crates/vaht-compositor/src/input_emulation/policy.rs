//! Which X programs may emulate input, decided from plain values.
//!
//! `[xwayland] input_emulation` names programs (by the name Xwayland gives
//! each, its command, such as `steam`) that may or may not, and what to do
//! with the rest: ask, allow or deny. An answer to a question lasts until
//! vaht exits. [`is_xwayland`] tells vaht's own Xwayland from any other
//! program that found the socket.

/// What happens to a program no list names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EmulationDefault {
    /// The user is asked, the first time it tries.
    #[default]
    Ask,
    /// It may, as on Xorg.
    Allow,
    /// It may not.
    Deny,
}

/// Which X programs may move the pointer and press keys through XTest
/// (Steam Input's desktop controls, xdotool, remote desktop servers).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InputEmulationSettings {
    /// What happens to a program no list names.
    pub(crate) default: EmulationDefault,
    /// Programs that may, without asking.
    pub(crate) allow: Vec<String>,
    /// Programs that may not, without asking.
    pub(crate) deny: Vec<String>,
}

/// Whether a program may emulate input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// It may.
    Allowed,
    /// It may not.
    Refused,
    /// The user decides.
    Ask,
}

/// The user's answers, kept until vaht exits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Answers {
    /// Each program answered for, and whether it may.
    given: Vec<(String, bool)>,
}

impl Answers {
    /// Notes that `program` may (`is_allowed`) or may not emulate input.
    pub(crate) fn note(&mut self, program: &str, is_allowed: bool) {
        self.given.retain(|(known, _)| known != program);
        self.given.push((program.to_owned(), is_allowed));
    }

    /// Returns the answer given for `program`, if one was.
    fn for_program(&self, program: &str) -> Option<bool> {
        self.given
            .iter()
            .find(|(known, _)| known == program)
            .map(|(_, is_allowed)| *is_allowed)
    }
}

impl InputEmulationSettings {
    /// Returns whether `program` may emulate input, after the user's
    /// `answers`: the deny list first, then the allow list, then an
    /// answer, then the default.
    pub(crate) fn admit(&self, program: &str, answers: &Answers) -> Admission {
        let is_named = |list: &[String]| list.iter().any(|named| named == program);
        if is_named(&self.deny) {
            return Admission::Refused;
        }
        if is_named(&self.allow) {
            return Admission::Allowed;
        }
        match (answers.for_program(program), self.default) {
            (Some(true), _) | (None, EmulationDefault::Allow) => Admission::Allowed,
            (Some(false), _) | (None, EmulationDefault::Deny) => Admission::Refused,
            (None, EmulationDefault::Ask) => Admission::Ask,
        }
    }
}

/// Returns whether `command_line` (a process's, as `/proc/<pid>/cmdline`
/// has it: arguments ending in NUL) is vaht's Xwayland on `display` (such
/// as `:1`): the program `Xwayland`, given the display first.
pub(crate) fn is_xwayland(command_line: &[u8], display: &str) -> bool {
    let mut arguments = command_line.split(|byte| *byte == 0);
    let program = arguments.next().unwrap_or_default();
    let name = program
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or_default();
    name == b"Xwayland" && arguments.next() == Some(display.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings defaulting to `default`, allowing `allow` and denying
    /// `deny`.
    fn settings(
        default: EmulationDefault,
        allow: &[&str],
        deny: &[&str],
    ) -> InputEmulationSettings {
        let owned = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        InputEmulationSettings {
            default,
            allow: owned(allow),
            deny: owned(deny),
        }
    }

    #[test]
    fn the_lists_come_before_answers_and_the_default() {
        let settings = settings(EmulationDefault::Ask, &["steam"], &["evil"]);
        let mut answers = Answers::default();
        answers.note("evil", true);
        assert_eq!(settings.admit("steam", &answers), Admission::Allowed);
        assert_eq!(
            settings.admit("evil", &answers),
            Admission::Refused,
            "denied by name"
        );
        assert_eq!(settings.admit("xdotool", &answers), Admission::Ask);
    }

    #[test]
    fn an_answer_lasts() {
        let settings = settings(EmulationDefault::Ask, &[], &[]);
        let mut answers = Answers::default();
        answers.note("xdotool", false);
        assert_eq!(settings.admit("xdotool", &answers), Admission::Refused);
        answers.note("xdotool", true);
        assert_eq!(
            settings.admit("xdotool", &answers),
            Admission::Allowed,
            "answered again"
        );
    }

    #[test]
    fn the_default_decides_the_rest() {
        let answers = Answers::default();
        assert_eq!(
            settings(EmulationDefault::Allow, &[], &[]).admit("any", &answers),
            Admission::Allowed
        );
        assert_eq!(
            settings(EmulationDefault::Deny, &[], &[]).admit("any", &answers),
            Admission::Refused
        );
    }

    #[test]
    fn only_vahts_xwayland_is_xwayland() {
        assert!(is_xwayland(b"/usr/bin/Xwayland\0:1\0-rootless\0", ":1"));
        assert!(is_xwayland(b"Xwayland\0:1\0", ":1"));
        assert!(
            !is_xwayland(b"/usr/bin/Xwayland\0:2\0", ":1"),
            "another display's"
        );
        assert!(!is_xwayland(b"/tmp/Xwayland-not\0:1\0", ":1"));
        assert!(!is_xwayland(b"python3\0Xwayland\0:1\0", ":1"));
        assert!(!is_xwayland(b"", ":1"));
    }
}
