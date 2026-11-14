//! Simulated monitors coming and going, for tests.
//!
//! With `VAHT_HEADLESS_OUTPUTS_FILE` set, the headless backend's outputs are
//! the ones that file lists, one `NAME WIDTHxHEIGHT` a line, and rewriting
//! it plugs in, unplugs and changes them as a real monitor would. What the
//! file asks for ([`OutputList`]) and what that changes ([`OutputList::changes_from`])
//! are plain values; the watch on the file only reads it in.

use std::path::{Path, PathBuf};

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::utils::{Physical, Size};

use crate::Compositor;
use crate::event_source::EventSource;
use crate::resources::ResourceLedger;

/// The outputs a file asks for, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OutputList {
    /// Each output's name and size.
    outputs: Vec<(String, Size<i32, Physical>)>,
}

/// What changes to make one list of outputs another.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OutputListChanges {
    /// The names of the outputs to remove.
    pub(crate) removed: Vec<String>,
    /// The outputs to add, by name and size.
    pub(crate) added: Vec<(String, Size<i32, Physical>)>,
    /// The outputs whose size changed, by name and new size.
    pub(crate) resized: Vec<(String, Size<i32, Physical>)>,
}

/// A watch on the outputs file; dropping it stops watching.
#[derive(Debug)]
pub(crate) struct OutputFileWatch {
    /// The inotify descriptor, registered with the event loop.
    _source: EventSource<Compositor>,
}

impl OutputList {
    /// Reads `text`: one `NAME WIDTHxHEIGHT` a line, blank lines and lines
    /// starting with `#` skipped.
    ///
    /// # Errors
    ///
    /// Returns the first line that is not one, or a name given twice.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut outputs: Vec<(String, Size<i32, Physical>)> = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let (Some(name), Some(size), None) = (words.next(), words.next(), words.next()) else {
                return Err(format!("\"{line}\" is not NAME WIDTHxHEIGHT"));
            };
            let size = size
                .split_once('x')
                .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
                .filter(|(width, height): &(i32, i32)| *width > 0 && *height > 0)
                .ok_or_else(|| format!("\"{size}\" is not a size such as 1920x1080"))?;
            if outputs.iter().any(|(known, _)| known == name) {
                return Err(format!("{name} is listed twice"));
            }
            outputs.push((name.to_owned(), size.into()));
        }
        Ok(Self { outputs })
    }

    /// Returns the outputs listed, in order.
    pub(crate) fn outputs(&self) -> &[(String, Size<i32, Physical>)] {
        &self.outputs
    }

    /// Returns what turns `current` (names and sizes) into this list.
    pub(crate) fn changes_from(
        &self,
        current: &[(String, Size<i32, Physical>)],
    ) -> OutputListChanges {
        let removed = current
            .iter()
            .filter(|(name, _)| !self.outputs.iter().any(|(wanted, _)| wanted == name))
            .map(|(name, _)| name.clone())
            .collect();
        let mut added = Vec::new();
        let mut resized = Vec::new();
        for (name, size) in &self.outputs {
            match current.iter().find(|(known, _)| known == name) {
                None => added.push((name.clone(), *size)),
                Some((_, known)) if known != size => resized.push((name.clone(), *size)),
                Some(_) => {}
            }
        }
        OutputListChanges {
            removed,
            added,
            resized,
        }
    }
}

impl OutputFileWatch {
    /// What happens to the file when a test rewrites it: written in place,
    /// or renamed over.
    const EVENTS: AddWatchFlags = AddWatchFlags::IN_CLOSE_WRITE.union(AddWatchFlags::IN_MOVED_TO);

    /// Starts watching the file at `path`; the compositor hears of each
    /// change.
    ///
    /// # Errors
    ///
    /// Returns why the file's directory cannot be watched.
    pub(crate) fn start(
        handle: &LoopHandle<'static, Compositor>,
        ledger: &ResourceLedger,
        path: &Path,
    ) -> Result<Self, String> {
        let directory = path
            .parent()
            .ok_or_else(|| format!("{} is in no directory", path.display()))?;
        let name = path
            .file_name()
            .ok_or_else(|| format!("{} names no file", path.display()))?
            .to_owned();
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(|error| format!("no inotify: {error}"))?;
        inotify
            .add_watch(directory, Self::EVENTS)
            .map_err(|error| format!("cannot watch {}: {error}", directory.display()))?;
        let path: PathBuf = path.to_owned();
        let source = EventSource::insert(
            handle,
            ledger,
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, compositor| {
                let Ok(events) = inotify.read_events() else {
                    return Ok(PostAction::Continue);
                };
                if events
                    .iter()
                    .any(|event| event.name.as_deref() == Some(name.as_os_str()))
                {
                    compositor.reread_headless_outputs(&path);
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| format!("cannot register the watch: {}", error.error))?;
        Ok(Self { _source: source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `(name, size)` for a list.
    fn entry(name: &str, width: i32, height: i32) -> (String, Size<i32, Physical>) {
        (name.to_owned(), Size::from((width, height)))
    }

    #[test]
    fn a_list_reads_one_output_a_line() {
        let list =
            OutputList::parse("# a desk\nLEFT 1920x1080\n\nRIGHT 1280x720\n").expect("valid");
        assert_eq!(
            list.outputs(),
            [entry("LEFT", 1920, 1080), entry("RIGHT", 1280, 720)]
        );
    }

    #[test]
    fn mistakes_in_a_list_are_explained() {
        assert!(OutputList::parse("LEFT").is_err());
        assert!(OutputList::parse("LEFT 0x10").is_err());
        assert!(OutputList::parse("LEFT 10x10\nLEFT 20x20").is_err());
    }

    #[test]
    fn changes_say_what_was_plugged_unplugged_and_resized() {
        let current = [entry("A", 800, 600), entry("B", 640, 480)];
        let wanted = OutputList::parse("B 1024x768\nC 320x240").expect("valid");
        assert_eq!(
            wanted.changes_from(&current),
            OutputListChanges {
                removed: vec!["A".to_owned()],
                added: vec![entry("C", 320, 240)],
                resized: vec![entry("B", 1024, 768)],
            }
        );
    }
}
