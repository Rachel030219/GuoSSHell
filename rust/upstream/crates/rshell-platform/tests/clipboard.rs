use rshell_platform::{ClipboardError, ClipboardPolicy};

#[test]
fn clipboard_text_normalizes_platform_newlines_without_changing_utf8() {
    let normalized =
        ClipboardPolicy::normalize_text("alpha\r\nbeta\rgamma\n界").expect("valid clipboard text");

    assert_eq!(normalized, "alpha\nbeta\ngamma\n界");
}

#[test]
fn clipboard_text_rejects_nul_and_exposes_stable_utf8_mime_priority() {
    assert_eq!(
        ClipboardPolicy::normalize_text("before\0after"),
        Err(ClipboardError::NulByte)
    );
    assert_eq!(
        ClipboardPolicy::text_mime_priority(),
        ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
    );
    assert!(!format!("{:?}", ClipboardError::NulByte).contains("before"));
}
