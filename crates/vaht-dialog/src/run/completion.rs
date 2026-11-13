//! Finishing the word before the cursor: program names for the first word,
//! file and directory names otherwise, as fbrun and fluxland-run do.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// Where names to complete come from: the directories programs are
/// looked for in, and the home directory `~` stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Completer {
    /// `PATH`'s directories, in order.
    program_directories: Vec<PathBuf>,
    /// The home directory.
    home: Option<PathBuf>,
}

impl Completer {
    /// Completes from `PATH` and `HOME`.
    pub(crate) fn from_environment() -> Self {
        let program_directories = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::new(program_directories, home)
    }

    /// Completes programs from `program_directories` and `~` as `home`.
    pub(crate) const fn new(program_directories: Vec<PathBuf>, home: Option<PathBuf>) -> Self {
        Self {
            program_directories,
            home,
        }
    }

    /// Returns what could finish `word`, sorted and each once: a program's
    /// name when `is_first` (the line's first word) and the word has no
    /// path in it, else a file or directory name, directories ending in
    /// `/`. A first word only completes to programs and directories.
    pub(crate) fn complete(&self, word: &str, is_first: bool) -> Vec<String> {
        let is_program_name = is_first && !word.contains('/') && !word.starts_with('~');
        let mut names = if is_program_name && !word.is_empty() {
            self.programs(word)
        } else if !word.is_empty() || !is_first {
            self.paths(word, is_first)
        } else {
            Vec::new()
        };
        names.sort();
        names.dedup();
        names
    }

    /// Returns the programs on `PATH` whose names start with `prefix`.
    fn programs(&self, prefix: &str) -> Vec<String> {
        self.program_directories
            .iter()
            .flat_map(|directory| {
                let directory = if directory.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    directory.as_path()
                };
                Self::names_in(directory, prefix)
                    .filter(|(_, path)| Self::is_program(path))
                    .map(|(name, _)| name)
            })
            .collect()
    }

    /// Returns the files and directories that finish `word`, a path as
    /// typed: its directory part kept as typed, `~` read as the home
    /// directory.
    fn paths(&self, word: &str, is_first: bool) -> Vec<String> {
        let split = word.rfind('/').map_or(0, |slash| slash + 1);
        let (typed_directory, prefix) = word.split_at(split);
        let read_directory = match typed_directory.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => match &self.home {
                Some(home) => home.join(rest.trim_start_matches('/')),
                None => return Vec::new(),
            },
            Some(_) | None if typed_directory.is_empty() => PathBuf::from("."),
            Some(_) | None => PathBuf::from(typed_directory),
        };
        Self::names_in(&read_directory, prefix)
            .filter_map(|(name, path)| {
                let is_directory = path.is_dir();
                // A program wants a program, or a directory to look in.
                if is_first && !is_directory && !Self::is_program(&path) {
                    return None;
                }
                let slash = if is_directory { "/" } else { "" };
                Some(format!("{typed_directory}{name}{slash}"))
            })
            .collect()
    }

    /// Returns the names in `directory` that start with `prefix`, with
    /// their paths; hidden ones only when `prefix` starts with a dot.
    fn names_in(directory: &Path, prefix: &str) -> impl Iterator<Item = (String, PathBuf)> {
        let shows_hidden = prefix.starts_with('.');
        let prefix = prefix.to_owned();
        fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .filter(move |(name, _)| {
                name.starts_with(&prefix) && (shows_hidden || !name.starts_with('.'))
            })
    }

    /// Returns whether `path` is a file someone may run.
    fn is_program(path: &Path) -> bool {
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
}

/// Returns the longest start all of `names` share.
pub(crate) fn common_prefix(names: &[String]) -> &str {
    let Some((first, rest)) = names.split_first() else {
        return "";
    };
    let length = rest.iter().fold(first.len(), |length, name| {
        first[..length]
            .char_indices()
            .zip(name.chars())
            .find(|((_, mine), theirs)| mine != theirs)
            .map_or(length.min(name.len()), |((index, _), _)| index)
    });
    &first[..length]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of programs `kitty` and `kill`, a plain file `kiwi`,
    /// and a home with `scm/`, `.hidden` and `notes.txt`.
    struct Setup {
        /// Holds everything.
        _root: tempfile::TempDir,
        /// The completer for it.
        completer: Completer,
    }

    impl Setup {
        /// Makes the files.
        fn new() -> Self {
            let root = tempfile::tempdir().expect("a temporary directory");
            let bin = root.path().join("bin");
            let home = root.path().join("home");
            for directory in [&bin, &home.join("scm")] {
                fs::create_dir_all(directory).expect("a directory");
            }
            for (path, mode) in [
                (bin.join("kitty"), 0o755),
                (bin.join("kill"), 0o755),
                (bin.join("kiwi"), 0o644),
                (home.join(".hidden"), 0o644),
                (home.join("notes.txt"), 0o644),
            ] {
                fs::write(&path, "").expect("a file");
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("a mode");
            }
            let completer = Completer::new(vec![bin.clone(), bin], Some(home));
            Self {
                _root: root,
                completer,
            }
        }
    }

    #[test]
    fn a_first_word_completes_to_programs_each_once() {
        let setup = Setup::new();
        assert_eq!(setup.completer.complete("ki", true), ["kill", "kitty"]);
    }

    #[test]
    fn later_words_complete_to_files_with_the_home_directory_kept_as_typed() {
        let setup = Setup::new();
        assert_eq!(
            setup.completer.complete("~/", false),
            ["~/notes.txt", "~/scm/"]
        );
        assert_eq!(setup.completer.complete("~/.h", false), ["~/.hidden"]);
    }

    #[test]
    fn a_first_word_with_a_path_completes_to_directories_and_programs_only() {
        let setup = Setup::new();
        assert_eq!(setup.completer.complete("~/", true), ["~/scm/"]);
    }

    #[test]
    fn an_empty_first_word_completes_to_nothing() {
        let setup = Setup::new();
        assert_eq!(setup.completer.complete("", true), Vec::<String>::new());
    }

    #[test]
    fn the_common_prefix_is_whole_characters() {
        let names = ["kitty".to_owned(), "kill".to_owned()];
        assert_eq!(common_prefix(&names), "ki");
        let names = ["é1".to_owned(), "é2".to_owned()];
        assert_eq!(common_prefix(&names), "é");
        assert_eq!(common_prefix(&["a".to_owned(), "ab".to_owned()]), "a");
        assert_eq!(common_prefix(&[]), "");
    }
}
