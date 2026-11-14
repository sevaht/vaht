//! The clipboard and the primary selection, as clipboard managers see them
//! through data control: what one program copies, another pastes.

use std::path::Path;

use vaht_test_support::{ClipboardManager, HeadlessVaht, Selection};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

#[test]
fn what_one_program_copies_another_pastes() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut copier = ClipboardManager::connect(vaht.socket());
    let mut reader = ClipboardManager::connect(vaht.socket());
    assert_eq!(reader.paste(Selection::Clipboard), None, "nothing yet");
    copier.copy(Selection::Clipboard, "copied text");
    let pasted = copier.answer_while(|| reader.paste(Selection::Clipboard));
    assert_eq!(pasted.as_deref(), Some("copied text"));
}

#[test]
fn the_primary_selection_is_apart_from_the_clipboard() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut copier = ClipboardManager::connect(vaht.socket());
    let mut reader = ClipboardManager::connect(vaht.socket());
    copier.copy(Selection::Primary, "selected text");
    let (primary, clipboard) = copier.answer_while(|| {
        (
            reader.paste(Selection::Primary),
            reader.paste(Selection::Clipboard),
        )
    });
    assert_eq!(primary.as_deref(), Some("selected text"));
    assert_eq!(clipboard, None);
}

#[test]
fn a_new_copy_replaces_the_last() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut first = ClipboardManager::connect(vaht.socket());
    let mut second = ClipboardManager::connect(vaht.socket());
    first.copy(Selection::Clipboard, "first");
    second.copy(Selection::Clipboard, "second");
    assert_eq!(
        second.paste(Selection::Clipboard).as_deref(),
        Some("second")
    );
    let pasted = second.answer_while(|| first.paste(Selection::Clipboard));
    assert_eq!(pasted.as_deref(), Some("second"));
    assert_eq!(
        first.replaced(),
        1,
        "the first copier heard it was replaced"
    );
}
