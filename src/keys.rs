use tao::keyboard::KeyCode;

/// The `KeyboardEvent.code` of a key Windows reports by its set-1 scancode, with
/// `0xE000` for an extended key — or `None` for a key the web has no code for, which is
/// left to Windows.
///
/// tao's tables are the UI Events code tables but for one name, which the web calls
/// Meta and tao calls Super.
pub fn dom_code(scancode: u32) -> Option<String> {
    match KeyCode::from_scancode(scancode) {
        KeyCode::Unidentified(_) => None,
        KeyCode::SuperLeft => Some("MetaLeft".into()),
        KeyCode::SuperRight => Some("MetaRight".into()),
        code => Some(format!("{code:?}")),
    }
}
