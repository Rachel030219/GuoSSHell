use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use rshell_platform::{
    ExternalEditorRequest, default_local_shell, external_editor_command, ssh_executable,
};
use serial_test::serial;

struct EnvRestore {
    name: &'static str,
    original: Option<OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: impl Into<OsString>) -> Self {
        let original = std::env::var_os(name);
        unsafe { std::env::set_var(name, value.into()) };
        Self { name, original }
    }

    fn remove(name: &'static str) -> Self {
        let original = std::env::var_os(name);
        unsafe { std::env::remove_var(name) };
        Self { name, original }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}

fn fixture_dir(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rshell-platform-{name}-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn editor_paths_with_spaces_are_arguments_not_a_shell_command() {
    let request = ExternalEditorRequest {
        editor: PathBuf::from("C:/Program Files/Microsoft VS Code/bin/code.cmd"),
        path: PathBuf::from("C:/work dir/notes;not-a-command.rs"),
        line: Some(42),
    };

    let command = external_editor_command(&request);
    assert_eq!(command.program, request.editor);
    assert_eq!(
        command.args,
        ["--goto", "C:/work dir/notes;not-a-command.rs:42"]
    );
    assert!(command.env.is_empty());
}

#[test]
fn non_vscode_editor_receives_only_the_path() {
    let request = ExternalEditorRequest {
        editor: PathBuf::from("vim"),
        path: PathBuf::from("a path/file.rs"),
        line: Some(42),
    };

    assert_eq!(external_editor_command(&request).args, ["a path/file.rs"]);
}

#[test]
#[serial]
fn shell_and_ssh_overrides_are_literal_program_paths() {
    let directory = fixture_dir("overrides with spaces");
    let shell = directory.join("custom shell.exe");
    let ssh = directory.join("custom ssh.exe");
    fs::write(&shell, []).unwrap();
    fs::write(&ssh, []).unwrap();
    let _shell = EnvRestore::set("RSHELL_SHELL", shell.as_os_str());
    let _ssh = EnvRestore::set("RSHELL_SSH", ssh.as_os_str());

    assert_eq!(default_local_shell().unwrap().program, shell);
    assert_eq!(ssh_executable().unwrap(), ssh);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[serial]
fn ssh_is_resolved_from_an_explicit_path_fixture() {
    let directory = fixture_dir("ssh path");
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let executable = directory.join(name);
    fs::write(&executable, []).unwrap();
    let _ssh = EnvRestore::remove("RSHELL_SSH");
    let _path = EnvRestore::set(
        "PATH",
        std::env::join_paths([Path::new(&directory)]).unwrap(),
    );

    assert_eq!(ssh_executable().unwrap(), executable);
    fs::remove_dir_all(directory).unwrap();
}
