//! Platform-specific facilities used by the rsHell workspace.

mod clipboard;
mod directories;
mod environment;
mod file_selection;
mod permissions;
mod process;
mod process_tree;
mod shell;

use std::io;

use thiserror::Error;

pub use clipboard::{ClipboardError, ClipboardPolicy};
pub use directories::PlatformPaths;
pub use environment::configure_runtime;
pub use file_selection::{
    FileSelectionCallback, FileSelectionError, FileSelectionPurpose, FileSelectionRequest,
    FileSelectionResult, FileSelectionService,
};
pub use permissions::{
    create_private_file, durable_replace_user_file, harden_private_file, private_file_is_secure,
};
pub use process::{CommandSpec, ExternalEditorRequest, external_editor_command, ssh_executable};
#[cfg(windows)]
pub use process_tree::WindowsProcessJob;
#[cfg(all(windows, feature = "containment-test-support"))]
pub use process_tree::{
    WindowsProcessJobTestFailure, WindowsProcessJobTestHook, WindowsProcessJobTestSnapshot,
};
pub use shell::{ShellSpec, default_local_shell};

/// Errors returned by the platform adapter without exposing environment values.
#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("platform application directories are unavailable")]
    DirectoriesUnavailable,
    #[error("{operation} failed")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("no usable {kind} executable was found")]
    ExecutableNotFound { kind: &'static str },
    #[error("the configured {kind} executable is not a file")]
    InvalidExecutable { kind: &'static str },
    #[error("Windows security operation failed")]
    Security,
    #[error("replacement files must be in the same directory")]
    ReplacementPathsMustBeSiblings,
}

impl PlatformError {
    pub(crate) fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }

    #[cfg(windows)]
    pub(crate) fn last_os_error() -> Self {
        Self::io("Windows platform operation", io::Error::last_os_error())
    }

    #[cfg(all(windows, feature = "containment-test-support"))]
    pub(crate) fn injected_containment_failure() -> Self {
        Self::io(
            "Windows platform operation",
            io::Error::other("injected containment boundary failure"),
        )
    }
}
