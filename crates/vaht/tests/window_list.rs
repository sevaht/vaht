//! The windows as docks and window switchers see them listed
//! (`ext_foreign_toplevel_list_v1`).

use std::path::Path;

use vaht_test_support::{HeadlessVaht, TestClient, WindowList};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

#[test]
fn a_dock_sees_windows_open_and_close() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut dock = WindowList::connect(vaht.socket());
    assert_eq!(dock.windows(), [], "no windows yet");
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    assert!(
        dock.wait_for(&[
            ("test window 1", "vaht.test"),
            ("test window 2", "vaht.test"),
        ]),
        "{:?}",
        dock.windows()
    );
    client.close_windows();
    assert!(dock.wait_for(&[]), "{:?}", dock.windows());
}

#[test]
fn a_dock_connecting_late_sees_the_windows_open() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    let mut dock = WindowList::connect(vaht.socket());
    assert!(
        dock.wait_for(&[("test window 1", "vaht.test")]),
        "{:?}",
        dock.windows()
    );
}
