#[cfg(windows)]
use std::{
    ffi::{c_char, c_void},
    path::Path,
};

use crate::{PlatformError, PlatformPaths};

/// Configures the process before GTK initialization, preserving actionable platform failures.
pub fn configure_runtime(_paths: &PlatformPaths) -> Result<(), PlatformError> {
    #[cfg(windows)]
    {
        configure_process_dpi_awareness();
        let executable = std::env::current_exe()
            .map_err(|error| PlatformError::io("locating current executable", error))?;
        if let Some(directory) = executable.parent() {
            configure_windows_portable_runtime(directory)?;
        }
        set_var("DBUS_SESSION_BUS_ADDRESS", "disabled:");
    }
    suppress_gio_warnings();
    Ok(())
}

#[cfg(windows)]
fn configure_process_dpi_awareness() {
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
    type SetProcessDpiAwarenessContext = unsafe extern "system" fn(isize) -> i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    unsafe {
        let user32 = LoadLibraryA(c"user32.dll".as_ptr());
        if user32.is_null() {
            return;
        }
        let procedure = GetProcAddress(user32, c"SetProcessDpiAwarenessContext".as_ptr());
        if procedure.is_null() {
            return;
        }
        let set_dpi_awareness: SetProcessDpiAwarenessContext = std::mem::transmute(procedure);
        let _ = set_dpi_awareness(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn suppress_gio_warnings() {
    unsafe { std::env::set_var("G_MESSAGES_DEBUG", "") };
}

#[cfg(windows)]
pub(crate) fn configure_windows_portable_runtime(directory: &Path) -> Result<(), PlatformError> {
    if has_gtk_runtime(directory) {
        prepend_path("PATH", directory)?;
    }

    let share = directory.join("share");
    if share.is_dir() {
        prepend_path("XDG_DATA_DIRS", &share)?;
        set_var("GTK_DATA_PREFIX", directory);
        set_var("GTK_EXE_PREFIX", directory);
    }
    set_path_if_exists(
        "GSETTINGS_SCHEMA_DIR",
        &share.join("glib-2.0").join("schemas"),
    );
    set_path_if_exists(
        "GDK_PIXBUF_MODULE_FILE",
        &directory
            .join("lib")
            .join("gdk-pixbuf-2.0")
            .join("2.10.0")
            .join("loaders.cache"),
    );
    set_path_if_exists(
        "GDK_PIXBUF_MODULEDIR",
        &directory
            .join("lib")
            .join("gdk-pixbuf-2.0")
            .join("2.10.0")
            .join("loaders"),
    );
    let font_config = directory.join("etc").join("fonts");
    set_path_if_exists("FONTCONFIG_FILE", &font_config.join("fonts.conf"));
    set_path_if_exists("FONTCONFIG_PATH", &font_config);
    Ok(())
}

#[cfg(windows)]
fn has_gtk_runtime(directory: &Path) -> bool {
    ["libgtk-4-1.dll", "libgtk-4-1-0.dll"]
        .into_iter()
        .any(|library| directory.join(library).is_file())
}

#[cfg(windows)]
fn prepend_path(name: &str, value: &Path) -> Result<(), PlatformError> {
    let mut entries: Vec<_> = match std::env::var_os(name) {
        Some(existing) => std::env::split_paths(&existing).collect(),
        None => Vec::new(),
    };
    if entries.iter().any(|entry| entry == value) {
        return Ok(());
    }
    entries.insert(0, value.to_path_buf());
    let joined = std::env::join_paths(entries).map_err(|error| {
        PlatformError::io(
            "updating runtime search path",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error),
        )
    })?;
    set_var(name, joined);
    Ok(())
}

#[cfg(windows)]
fn set_path_if_exists(name: &str, value: &Path) {
    if value.exists() {
        set_var(name, value);
    }
}

#[cfg(windows)]
fn set_var(name: &str, value: impl AsRef<std::ffi::OsStr>) {
    unsafe { std::env::set_var(name, value) };
}

#[cfg(all(test, windows))]
mod tests {
    use std::{ffi::OsString, fs};

    use serial_test::serial;

    use super::configure_windows_portable_runtime;

    struct EnvRestore(&'static str, Option<OsString>);

    impl EnvRestore {
        fn set(name: &'static str, value: impl Into<OsString>) -> Self {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value.into()) };
            Self(name, previous)
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.1 {
                    Some(value) => std::env::set_var(self.0, value),
                    None => std::env::remove_var(self.0),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn portable_runtime_paths_are_prepended_once() {
        let root = std::env::temp_dir().join(format!("rshell-runtime-{}", std::process::id()));
        fs::create_dir_all(root.join("share")).unwrap();
        fs::write(root.join("libgtk-4-1.dll"), []).unwrap();
        let _path = EnvRestore::set("PATH", "");

        configure_windows_portable_runtime(&root).unwrap();
        let once = std::env::var_os("PATH").unwrap();
        configure_windows_portable_runtime(&root).unwrap();

        assert_eq!(std::env::var_os("PATH"), Some(once));
        assert_eq!(
            std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .filter(|entry| entry == &root)
                .count(),
            1,
        );
        fs::remove_dir_all(root).unwrap();
    }
}
