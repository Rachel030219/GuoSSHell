use std::{
    fs::{File, OpenOptions},
    path::Path,
};

#[cfg(not(windows))]
use std::fs;

use crate::PlatformError;

#[cfg(windows)]
mod windows;

/// Creates a new file whose permissions are restricted to its owner and system services.
#[cfg(windows)]
pub fn create_private_file(path: &Path) -> Result<File, PlatformError> {
    windows::create_private_file(path)
}

/// Replaces an existing file's discretionary ACL with the private rsHell ACL.
#[cfg(windows)]
pub fn harden_private_file(path: &Path) -> Result<(), PlatformError> {
    windows::harden_private_file(path)
}

/// Checks whether an existing file has the private rsHell DACL.
#[cfg(windows)]
pub fn private_file_is_secure(path: &Path) -> Result<bool, PlatformError> {
    windows::private_file_is_secure(path)
}

/// Creates a new file with mode 0600.
#[cfg(unix)]
pub fn create_private_file(path: &Path) -> Result<File, PlatformError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| PlatformError::io("creating private file", error))
}

/// Forces an existing file to mode 0600.
#[cfg(unix)]
pub fn harden_private_file(path: &Path) -> Result<(), PlatformError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| PlatformError::io("hardening private file", error))
}

/// Returns true only when group and world permissions are absent.
#[cfg(unix)]
pub fn private_file_is_secure(path: &Path) -> Result<bool, PlatformError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| PlatformError::io("reading private file permissions", error))?
        .permissions()
        .mode();
    Ok(mode & 0o077 == 0 && mode & 0o600 == 0o600)
}

#[cfg(not(any(unix, windows)))]
pub fn create_private_file(path: &Path) -> Result<File, PlatformError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| PlatformError::io("creating private file", error))
}

#[cfg(not(any(unix, windows)))]
pub fn harden_private_file(_path: &Path) -> Result<(), PlatformError> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn private_file_is_secure(_path: &Path) -> Result<bool, PlatformError> {
    Ok(false)
}

/// Atomically replaces a private application file with a flushed sibling temporary file.
///
/// `source` and `destination` must have the same parent so replacement never degrades into a
/// cross-filesystem copy. The replacement source is hardened first; an existing destination is
/// hardened before replacement because `ReplaceFileW` retains its DACL. The resulting destination
/// is then hardened and verified. Content replacement is atomic, but ACL hardening and replacement
/// use separate platform APIs.
pub fn durable_replace_user_file(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    let Some(source_parent) = source.parent() else {
        return Err(PlatformError::ReplacementPathsMustBeSiblings);
    };
    let Some(destination_parent) = destination.parent() else {
        return Err(PlatformError::ReplacementPathsMustBeSiblings);
    };
    if source_parent != destination_parent {
        return Err(PlatformError::ReplacementPathsMustBeSiblings);
    }

    OpenOptions::new()
        .write(true)
        .open(source)
        .and_then(|file| file.sync_all())
        .map_err(|error| PlatformError::io("flushing replacement file", error))?;

    harden_replacement_inputs(source, destination)?;
    replace_file(source, destination)?;
    harden_private_file(destination)?;

    #[cfg(any(unix, windows))]
    if !private_file_is_secure(destination)? {
        return Err(PlatformError::Security);
    }

    #[cfg(unix)]
    {
        File::open(destination_parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| PlatformError::io("syncing replacement directory", error))?;
    }

    Ok(())
}

fn harden_replacement_inputs(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    harden_private_file(source)?;
    if destination
        .try_exists()
        .map_err(|error| PlatformError::io("checking replacement destination", error))?
    {
        harden_private_file(destination)?;
    }
    Ok(())
}

#[cfg(unix)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    fs::rename(source, destination)
        .map_err(|error| PlatformError::io("replacing private file", error))
}

#[cfg(all(test, windows))]
mod tests {
    use std::{fs, io::Write};

    use super::*;

    #[test]
    fn hardening_replacement_inputs_secures_an_existing_destination_before_replace() {
        let root = std::env::temp_dir().join(format!(
            "rshell-platform-replace-order-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        let mut source_file = create_private_file(&source).unwrap();
        source_file.write_all(b"replacement").unwrap();
        drop(source_file);
        fs::write(&destination, b"old").unwrap();
        windows::make_insecure_for_test(&destination).unwrap();
        assert!(!private_file_is_secure(&destination).unwrap());

        harden_replacement_inputs(&source, &destination).unwrap();

        assert!(private_file_is_secure(&source).unwrap());
        assert!(private_file_is_secure(&destination).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    windows::replace_file(source, destination)
}

#[cfg(not(any(unix, windows)))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    fs::rename(source, destination)
        .map_err(|error| PlatformError::io("replacing private file", error))
}
