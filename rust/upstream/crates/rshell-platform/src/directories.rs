use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;

use crate::PlatformError;

/// Application-specific locations for persistent platform data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPaths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl PlatformPaths {
    /// Discovers standard application directories for rsHell on this platform.
    pub fn discover() -> Result<Self, PlatformError> {
        let directories = ProjectDirs::from("io.github.hugefiver", "hugefiver", "rshell")
            .ok_or(PlatformError::DirectoriesUnavailable)?;
        let state_dir = directories
            .state_dir()
            .unwrap_or_else(|| directories.data_local_dir())
            .to_path_buf();

        Ok(Self::from_roots(
            directories.config_dir(),
            state_dir,
            directories.cache_dir(),
        ))
    }

    /// Creates paths from supplied roots without reading process environment variables.
    pub fn from_roots(
        config_dir: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
        cache_dir: impl AsRef<Path>,
    ) -> Self {
        Self {
            config_dir: config_dir.as_ref().to_path_buf(),
            state_dir: state_dir.as_ref().to_path_buf(),
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    /// Creates every location atomically where supported and safely when repeated.
    pub fn ensure_exists(&self) -> Result<(), PlatformError> {
        for directory in [&self.config_dir, &self.state_dir, &self.cache_dir] {
            fs::create_dir_all(directory)
                .map_err(|error| PlatformError::io("creating platform directory", error))?;
        }
        Ok(())
    }

    /// Returns rsHell's application-owned known-hosts file, never the user's OpenSSH file.
    pub fn known_hosts_path(&self) -> PathBuf {
        self.config_dir.join("known_hosts")
    }
}
