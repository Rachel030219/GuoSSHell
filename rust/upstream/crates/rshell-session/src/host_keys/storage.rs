use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use rshell_platform::{create_private_file, durable_replace_user_file, harden_private_file};
use russh::keys::{PublicKey, known_hosts};

use super::{HostKeyError, HostKeyStorageStep};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn store(
    destination: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), HostKeyError> {
    store_with_copy(destination, host, port, key, copy_existing_file)
}

fn store_with_copy(
    destination: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
    copy: impl FnOnce(&Path, &mut File) -> io::Result<()>,
) -> Result<(), HostKeyError> {
    let Some(parent) = destination.parent() else {
        return Err(storage_error(host, port, HostKeyStorageStep::CreateParent));
    };
    fs::create_dir_all(parent)
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::CreateParent))?;

    let (temporary_path, temporary_file) = create_private_temporary_file(parent)
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::CreateTemporary))?;
    let temporary = TemporaryKnownHostsFile(temporary_path);
    {
        let mut temporary_file = temporary_file;
        copy(destination, &mut temporary_file)
            .map_err(|_| storage_error(host, port, HostKeyStorageStep::CopyExisting))?;
        temporary_file
            .flush()
            .and_then(|()| temporary_file.sync_all())
            .map_err(|_| storage_error(host, port, HostKeyStorageStep::SyncTemporary))?;
    }

    known_hosts::learn_known_hosts_path(host, port, key, temporary.path())
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::Learn))?;
    OpenOptions::new()
        .write(true)
        .open(temporary.path())
        .and_then(|file| file.sync_all())
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::SyncLearned))?;
    harden_private_file(temporary.path())
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::HardenTemporary))?;
    durable_replace_user_file(temporary.path(), destination)
        .map_err(|_| storage_error(host, port, HostKeyStorageStep::Replace))?;
    Ok(())
}

fn storage_error(host: &str, port: u16, step: HostKeyStorageStep) -> HostKeyError {
    HostKeyError::storage(host, port, step)
}

fn copy_existing_file(path: &Path, target: &mut File) -> io::Result<()> {
    match File::open(path) {
        Ok(mut source) => {
            io::copy(&mut source, target)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_private_temporary_file(
    parent: &Path,
) -> Result<(std::path::PathBuf, File), rshell_platform::PlatformError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = parent.join(format!(
        ".rshell-known-hosts-{}-{sequence}.tmp",
        std::process::id()
    ));
    create_private_file(&path).map(|file| (path, file))
}

struct TemporaryKnownHostsFile(std::path::PathBuf);

impl TemporaryKnownHostsFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryKnownHostsFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::parse_public_key_base64;
    use tempfile::TempDir;

    const KEY: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ";

    #[test]
    fn copy_failure_preserves_destination_and_removes_private_temporary_file() {
        let temp = TempDir::new().unwrap();
        let destination = temp.path().join("known_hosts");
        fs::write(&destination, b"original\n").unwrap();
        let key = parse_public_key_base64(KEY).unwrap();

        let error = store_with_copy(&destination, "storage.test", 22, &key, |_, target| {
            target.write_all(b"partial")?;
            Err(io::Error::other("injected copy failure"))
        })
        .expect_err("copy failure must abort learning");

        assert!(matches!(
            error,
            HostKeyError::Storage {
                step: HostKeyStorageStep::CopyExisting,
                ..
            }
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"original\n");
        let entries = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, ["known_hosts"]);
    }
}
