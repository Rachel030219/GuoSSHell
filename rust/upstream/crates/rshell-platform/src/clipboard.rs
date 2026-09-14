use std::fmt;

/// Platform-neutral validation performed before terminal clipboard text reaches GTK.
pub struct ClipboardPolicy;

#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClipboardError {
    #[error("clipboard text contains a NUL byte")]
    NulByte,
}

impl fmt::Debug for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClipboardError::NulByte")
    }
}

impl ClipboardPolicy {
    const TEXT_MIME_PRIORITY: [&'static str; 3] =
        ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

    pub fn normalize_text(text: &str) -> Result<String, ClipboardError> {
        if text.contains('\0') {
            return Err(ClipboardError::NulByte);
        }
        Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
    }

    pub const fn text_mime_priority() -> [&'static str; 3] {
        Self::TEXT_MIME_PRIORITY
    }
}
