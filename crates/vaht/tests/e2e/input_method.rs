//! Input methods (`zwp_input_method_v2`) typing into the focused window's
//! text field (`zwp_text_input_v3`), as fcitx5 or ibus does.

use std::path::Path;

use vaht_test_support::{HeadlessVaht, InputMethod, TestClient};

/// The executable cargo built for these tests.
const EXECUTABLE: &str = env!("CARGO_BIN_EXE_vaht");

#[test]
fn an_input_method_types_into_the_focused_window() {
    let vaht = HeadlessVaht::start(Path::new(EXECUTABLE));
    let mut method = InputMethod::connect(vaht.socket());
    let mut client = TestClient::connect(vaht.socket());
    client.open_windows(1);
    assert!(
        method.wait_until_active(),
        "the focused window's text field is active"
    );
    method.type_text("こんにちは");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while client.typed_text().is_empty() && std::time::Instant::now() < deadline {}
    assert_eq!(client.typed_text(), "こんにちは");
}
