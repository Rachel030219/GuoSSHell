use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSelectionPurpose {
    LegacyRshellImport,
    OpenSshImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSelectionRequest {
    pub purpose: FileSelectionPurpose,
    pub title: &'static str,
}

impl FileSelectionRequest {
    pub const fn legacy_import() -> Self {
        Self {
            purpose: FileSelectionPurpose::LegacyRshellImport,
            title: "Select legacy rsHell connections",
        }
    }

    pub const fn open_ssh_import() -> Self {
        Self {
            purpose: FileSelectionPurpose::OpenSshImport,
            title: "Select OpenSSH configuration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSelectionError {
    Unavailable,
    InvalidSelection,
}

impl std::fmt::Display for FileSelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "file selection is unavailable",
            Self::InvalidSelection => "the selected file is unavailable",
        })
    }
}

impl std::error::Error for FileSelectionError {}

pub type FileSelectionResult = Result<Option<PathBuf>, FileSelectionError>;
pub type FileSelectionCallback = Box<dyn FnOnce(FileSelectionResult) + 'static>;

/// Asynchronous, UI-agnostic boundary for selecting one user file.
pub trait FileSelectionService: 'static {
    fn select_file(&self, request: FileSelectionRequest, complete: FileSelectionCallback);
}
