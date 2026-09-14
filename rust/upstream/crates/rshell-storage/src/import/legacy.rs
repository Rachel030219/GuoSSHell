use std::{
    fs,
    path::{Path, PathBuf},
};

use secrecy::SecretString;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use super::{ImportError, ImportPreview, ImportWarning, legacy_mapping};

#[derive(Debug, Clone, Copy)]
pub struct LegacyJsonImporter;

#[derive(Deserialize)]
pub(super) struct LegacyDocument {
    #[serde(default)]
    pub(super) folders: Vec<LegacyFolder>,
    #[serde(default)]
    pub(super) connections: Vec<LegacyConnection>,
}

#[derive(Deserialize)]
pub(super) struct LegacyFolder {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Deserialize)]
pub(super) struct LegacyConnection {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) folder_id: Option<String>,
    pub(super) host: String,
    #[serde(default)]
    pub(super) port: Option<u64>,
    #[serde(default)]
    pub(super) user: String,
    #[serde(default, deserialize_with = "password")]
    pub(super) password: Option<SecretString>,
    #[serde(default)]
    pub(super) identity_file: String,
    #[serde(default)]
    pub(super) remote_command: String,
    #[serde(default)]
    pub(super) note: String,
    #[serde(default = "default_backend")]
    pub(super) backend: String,
    #[serde(default = "default_accept_new_host")]
    pub(super) accept_new_host: bool,
    #[serde(default)]
    pub(super) terminal: LegacyTerminal,
}

#[derive(Default, Deserialize)]
pub(super) struct LegacyTerminal {
    #[serde(default, alias = "type")]
    pub(super) terminal_type: Option<String>,
    #[serde(default, alias = "cols")]
    pub(super) initial_cols: Option<u16>,
    #[serde(default, alias = "rows")]
    pub(super) initial_rows: Option<u16>,
    #[serde(default, alias = "scrollback")]
    pub(super) scrollback_lines: Option<usize>,
    #[serde(default)]
    pub(super) delete_key: Option<String>,
    #[serde(default)]
    pub(super) backspace_key: Option<String>,
    #[serde(default)]
    pub(super) left_alt_as_meta: Option<bool>,
    #[serde(default)]
    pub(super) right_alt_as_meta: Option<bool>,
    #[serde(default)]
    pub(super) enable_csi_u: Option<bool>,
    #[serde(default)]
    pub(super) enable_kitty_keyboard: Option<bool>,
    #[serde(default)]
    pub(super) enable_kitty_graphics: Option<bool>,
    #[serde(default)]
    pub(super) mouse_reporting: Option<bool>,
    #[serde(default)]
    pub(super) scroll_on_output: Option<bool>,
    #[serde(default)]
    pub(super) scroll_on_keypress: Option<bool>,
    #[serde(default)]
    pub(super) answerback: Option<String>,
    #[serde(default, alias = "color")]
    pub(super) color_scheme: Option<String>,
    #[serde(default, alias = "font")]
    pub(super) font_size: Option<u16>,
}

impl LegacyJsonImporter {
    pub const fn new() -> Self {
        Self
    }

    pub fn preview(&self, path: impl AsRef<Path>) -> Result<ImportPreview, ImportError> {
        let path = path.as_ref();
        match preview_source(path) {
            Ok(preview) => Ok(preview),
            Err(primary_error) => {
                let backup = backup_path(path);
                if !backup.is_file() {
                    return Err(primary_error);
                }
                match preview_source(&backup) {
                    Ok(mut preview) => {
                        preview.warnings.push(ImportWarning::RecoveredFromBackup);
                        Ok(preview)
                    }
                    Err(_) => Err(ImportError::NoUsableSource),
                }
            }
        }
    }
}

fn preview_source(path: &Path) -> Result<ImportPreview, ImportError> {
    let mut source = fs::read(path).map_err(|_| ImportError::Io)?;
    let digest = Sha256::digest(&source);
    let document =
        serde_json::from_slice::<LegacyDocument>(&source).map_err(|_| ImportError::InvalidJson);
    source.zeroize();
    legacy_mapping::map_document(document?, hex_digest(&digest))
}

fn password<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.map(SecretString::from))
}

fn default_backend() -> String {
    "system_open_ssh".into()
}

const fn default_accept_new_host() -> bool {
    true
}

fn backup_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    parent.join(format!("{name}.bak"))
}

fn hex_digest(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Default for LegacyJsonImporter {
    fn default() -> Self {
        Self::new()
    }
}
