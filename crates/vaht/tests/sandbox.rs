//! Sandboxed programs (`wp_security_context_v1`): a sandbox engine's
//! socket, and what programs connecting there are offered.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use vaht_test_support::{HeadlessVaht, Sandbox};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

/// What only unrestricted programs are offered.
const PRIVILEGED: [&str; 17] = [
    "ext_data_control_manager_v1",
    "zwlr_data_control_manager_v1",
    "ext_foreign_toplevel_list_v1",
    "ext_session_lock_manager_v1",
    "ext_output_image_capture_source_manager_v1",
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "ext_image_copy_capture_manager_v1",
    "zwlr_virtual_pointer_manager_v1",
    "zwp_virtual_keyboard_manager_v1",
    "zwlr_layer_shell_v1",
    "wp_security_context_manager_v1",
    "zwlr_foreign_toplevel_manager_v1",
    "zwp_input_method_manager_v2",
    "zwlr_output_power_manager_v1",
    "zwlr_gamma_control_manager_v1",
    "zwlr_output_manager_v1",
    "ext_workspace_manager_v1",
];

#[test]
fn a_sandboxed_program_is_offered_windows_but_not_the_privileged_globals() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let everyone = Sandbox::globals(vaht.socket());
    for name in PRIVILEGED {
        assert!(
            everyone.iter().any(|global| global == name),
            "{name} is offered: {everyone:?}"
        );
    }
    let mut sandbox = Sandbox::create(vaht.socket(), "test");
    let sandboxed = Sandbox::globals(&sandbox.socket());
    for name in [
        "wl_compositor",
        "xdg_wm_base",
        "wl_seat",
        "wl_shm",
        "zwp_tablet_manager_v2",
        "zwp_text_input_manager_v3",
    ] {
        assert!(
            sandboxed.iter().any(|global| global == name),
            "{name}: {sandboxed:?}"
        );
    }
    for name in PRIVILEGED {
        assert!(
            !sandboxed.iter().any(|global| global == name),
            "{name} is hidden: {sandboxed:?}"
        );
    }
    vaht.wait_for_logged("a sandbox's programs may connect", 1);
    sandbox.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(sandbox.socket()).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        UnixStream::connect(sandbox.socket()).is_err(),
        "a closed sandbox's socket is no longer listened on"
    );
}
