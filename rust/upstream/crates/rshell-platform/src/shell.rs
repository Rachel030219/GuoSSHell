use std::{collections::BTreeMap, env, ffi::OsString, path::PathBuf};

use crate::PlatformError;

#[cfg(windows)]
use crate::process::resolve_executable;

/// A shell launch description that never requires a command-line parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
}

/// Returns the configured local interactive shell without launching it.
#[cfg(windows)]
pub fn default_local_shell() -> Result<ShellSpec, PlatformError> {
    if let Some(program) = environment_program("RSHELL_SHELL") {
        return Ok(shell_spec(program));
    }

    for candidate in ["pwsh.exe", "powershell.exe"] {
        if let Some(program) = resolve_executable(candidate) {
            return Ok(shell_spec(program));
        }
    }
    if let Some(program) = environment_program("COMSPEC").filter(|path| path.is_file()) {
        return Ok(shell_spec(program));
    }
    Err(PlatformError::ExecutableNotFound {
        kind: "local shell",
    })
}

/// Returns the configured local interactive shell without launching it.
#[cfg(not(windows))]
pub fn default_local_shell() -> Result<ShellSpec, PlatformError> {
    Ok(shell_spec(
        environment_program("RSHELL_SHELL")
            .or_else(|| environment_program("SHELL"))
            .unwrap_or_else(|| PathBuf::from("/bin/sh")),
    ))
}

fn shell_spec(program: PathBuf) -> ShellSpec {
    #[cfg(windows)]
    let args = vec![OsString::from("-NoLogo")];
    #[cfg(not(windows))]
    let args = Vec::new();

    ShellSpec {
        program,
        args,
        env: BTreeMap::new(),
    }
}

pub(crate) fn environment_program(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
