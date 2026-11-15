//! Pipe menus: a submenu whose items a command prints each time it opens,
//! as Openbox has them (`{ label = "Recent", pipe = "~/bin/recent" }`).
//!
//! The command runs with the shell, its output read on the event loop as
//! it comes ([`PipeRead`]), so a slow one never stalls vaht; the submenu
//! shows "…" meanwhile. When the command ends, what it printed is read as
//! a menu ([`parse_printed`]): TOML with `items`, as a `[menu.<name>]`
//! table holds them, and optionally a `title`. A menu closed meanwhile is
//! not filled.

use std::fs::File;
use std::io::Read as _;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::Compositor;
use crate::config::MenuDefinition;
use crate::event_source::EndingSource;
use crate::resources::ResourceLedger;

/// What a pipe menu's command printed so far.
#[derive(Debug)]
pub(crate) struct PipeRead {
    /// The pipe menu's number.
    number: u64,
    /// What was read.
    output: Vec<u8>,
    /// Whether the command printed more than a menu may be.
    is_too_long: bool,
    /// Reads the command's output while it lives.
    _source: EndingSource<Compositor>,
}

impl PipeRead {
    /// The most a pipe menu's command may print: far more than any menu
    /// needs, so one printing more is broken, and is not kept.
    pub(crate) const MAX_BYTES: usize = 256 * 1024;

    /// Reads `reader`, the output of pipe menu `number`'s command, on
    /// `event_loop`; the compositor is told when it ends.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader cannot be registered.
    pub(crate) fn start(
        number: u64,
        reader: File,
        event_loop: (&LoopHandle<'static, Compositor>, &ResourceLedger),
    ) -> Result<Self, calloop::Error> {
        let (handle, ledger) = event_loop;
        let source = Generic::new(reader, Interest::READ, Mode::Level);
        let source = EndingSource::insert(handle, ledger, source, move |_, reader, compositor| {
            let mut chunk = [0; 4096];
            let read = (&mut &**reader).read(&mut chunk);
            let is_ended = match read {
                Ok(0) | Err(_) => true,
                Ok(count) => !compositor.menus.pipe_output(number, &chunk[..count]),
            };
            if is_ended {
                compositor.finish_pipe_menu(number);
                Ok(PostAction::Remove)
            } else {
                Ok(PostAction::Continue)
            }
        })?;
        Ok(Self {
            number,
            output: Vec::new(),
            is_too_long: false,
            _source: source,
        })
    }

    /// Keeps `bytes`, more of pipe menu `number`'s output; returns whether
    /// to read on: not past [`Self::MAX_BYTES`], nor for another menu.
    pub(crate) fn take_in(&mut self, number: u64, bytes: &[u8]) -> bool {
        if number != self.number {
            return false;
        }
        if self.output.len() + bytes.len() > Self::MAX_BYTES {
            self.is_too_long = true;
            return false;
        }
        self.output.extend_from_slice(bytes);
        true
    }

    /// Returns what pipe menu `number`'s command printed, once; an error if
    /// it printed too much.
    pub(crate) fn take_output(&mut self, number: u64) -> Option<Result<Vec<u8>, String>> {
        if number != self.number {
            return None;
        }
        if self.is_too_long {
            return Some(Err(format!(
                "it printed more than {} bytes",
                Self::MAX_BYTES
            )));
        }
        Some(Ok(std::mem::take(&mut self.output)))
    }
}

/// Reads what a pipe menu's command printed as a menu.
///
/// # Errors
///
/// Returns what is wrong with it.
pub(crate) fn parse_printed(output: &[u8]) -> Result<MenuDefinition, String> {
    let text = std::str::from_utf8(output).map_err(|_not_text| "it printed no text".to_owned())?;
    toml::from_str(text).map_err(|error| error.message().trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MenuEntry;

    #[test]
    fn printed_items_are_read_as_a_menus_are() {
        let menu = parse_printed(
            b"title = \"Recent\"\nitems = [{ label = \"a.txt\", command = \"exec xdg-open a.txt\" }]\n",
        )
        .expect("a menu");
        assert_eq!(menu.title.as_deref(), Some("Recent"));
        assert!(matches!(&menu.items[..], [MenuEntry::Command { label, .. }] if label == "a.txt"));
        assert!(parse_printed(b"items = [{ wrong = 1 }]").is_err());
        assert!(parse_printed(&[0xff, 0xfe]).is_err());
    }
}
