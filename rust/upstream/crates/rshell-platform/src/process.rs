use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use crate::{PlatformError, shell::environment_program};

/// A process description with pre-separated argv and environment values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
}

/// A request to open a local path in an external editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEditorRequest {
    pub editor: PathBuf,
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// Locates the system SSH client without invoking a shell.
pub fn ssh_executable() -> Result<PathBuf, PlatformError> {
    if let Some(override_program) = environment_program("RSHELL_SSH") {
        return resolve_executable(&override_program)
            .ok_or(PlatformError::InvalidExecutable { kind: "SSH" });
    }

    for candidate in ssh_candidates() {
        if let Some(program) = resolve_executable(candidate) {
            return Ok(program);
        }
    }
    Err(PlatformError::ExecutableNotFound { kind: "SSH" })
}

/// Builds an editor command without launching the editor or a command shell.
pub fn external_editor_command(request: &ExternalEditorRequest) -> CommandSpec {
    let args = match (is_vscode(&request.editor), request.line) {
        (true, Some(line)) => vec![OsString::from("--goto"), goto_argument(&request.path, line)],
        _ => vec![request.path.as_os_str().to_os_string()],
    };
    CommandSpec {
        program: request.editor.clone(),
        args,
        env: BTreeMap::new(),
    }
}

pub(crate) fn resolve_executable(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = PathBuf::from(program.as_ref());
    if program.is_absolute() || program.components().count() > 1 {
        return program.is_file().then_some(program);
    }

    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(&program))
        .find(|candidate| candidate.is_file())
}

fn ssh_candidates() -> &'static [&'static str] {
    #[cfg(windows)]
    return &["ssh.exe", "ssh"];
    #[cfg(not(windows))]
    &["ssh"]
}

fn is_vscode(editor: &Path) -> bool {
    let Some(name) = editor.file_stem() else {
        return false;
    };
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case("code") || name.eq_ignore_ascii_case("code-insiders")
}

fn goto_argument(path: &Path, line: u32) -> OsString {
    let mut argument = path.as_os_str().to_os_string();
    argument.push(":");
    argument.push(line.to_string());
    argument
}
