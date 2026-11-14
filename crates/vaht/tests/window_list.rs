//! The windows as docks and window switchers see them listed
//! (`ext_foreign_toplevel_list_v1`), and as taskbars outside vaht see and
//! work them (`zwlr_foreign_toplevel_manager_v1`).

use std::path::Path;

use vaht_test_support::{
    HeadlessVaht, ManagedWindow, PagedWorkspace, Pager, TestClient, ToplevelManager, WindowList,
};

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

#[test]
fn a_taskbar_outside_vaht_sees_and_works_the_windows() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut manager = ToplevelManager::connect(vaht.socket());
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(2);
    let states = |windows: &[ManagedWindow]| -> Vec<Vec<&'static str>> {
        windows.iter().map(|window| window.states.clone()).collect()
    };
    let windows =
        manager.wait_for(|windows| windows.len() == 2 && windows[1].states == ["activated"]);
    assert_eq!(
        states(&windows),
        [vec![], vec!["activated"]],
        "the second has focus"
    );

    manager.activate(0);
    let windows = manager.wait_for(|windows| windows[0].states == ["activated"]);
    assert_eq!(states(&windows), [vec!["activated"], vec![]]);

    manager.maximize(0);
    let windows = manager.wait_for(|windows| windows[0].states.contains(&"maximized"));
    assert_eq!(states(&windows)[0], ["maximized", "activated"]);

    manager.minimize(1);
    let windows = manager.wait_for(|windows| windows[1].states.contains(&"minimized"));
    assert_eq!(states(&windows)[1], ["minimized"]);

    manager.close(0);
    assert!(
        client.wait_for_close_requests(1),
        "the window was asked to close"
    );
}

#[test]
fn a_pager_outside_vaht_sees_and_switches_the_workspaces() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut pager = Pager::connect(vaht.socket());
    let shown =
        |workspaces: &[PagedWorkspace]| workspaces.iter().position(|(_, is_active)| *is_active);
    let workspaces = pager.wait_for(|workspaces| workspaces.len() == 4);
    assert_eq!(workspaces[0], ("Workspace 1".to_owned(), true));
    assert_eq!(shown(&workspaces), Some(0));

    pager.activate(2);
    let workspaces = pager.wait_for(|workspaces| shown(workspaces) == Some(2));
    assert_eq!(shown(&workspaces), Some(2));

    pager.create("Extra");
    let workspaces = pager.wait_for(|workspaces| workspaces.len() == 5);
    assert_eq!(workspaces[4], ("Extra".to_owned(), false));

    pager.remove(0);
    pager.remove(4);
    let workspaces = pager.wait_for(|workspaces| workspaces.len() == 4);
    let names: Vec<&str> = workspaces.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["Workspace 1", "Workspace 2", "Workspace 3", "Workspace 4"],
        "only the last can be removed"
    );
}
