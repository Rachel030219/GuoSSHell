use std::path::{Path, PathBuf};

use rshell_core::{
    AppSettings, AuthenticationKind, ColorScheme, ConnectionId, GroupId, HostKeyPolicy, KeyBinding,
    TerminalOverrides, TerminalProfileId, TransportKind,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StorageError;

pub(crate) fn uuid_text(value: Uuid) -> String {
    value.hyphenated().to_string()
}

fn parse_uuid(value: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(value).map_err(|_| StorageError::Corrupt)
}

pub(crate) fn connection_id(value: &str) -> Result<ConnectionId, StorageError> {
    Ok(ConnectionId(parse_uuid(value)?))
}

pub(crate) fn group_id(value: &str) -> Result<GroupId, StorageError> {
    Ok(GroupId(parse_uuid(value)?))
}

pub(crate) fn profile_id(value: &str) -> Result<TerminalProfileId, StorageError> {
    Ok(TerminalProfileId(parse_uuid(value)?))
}

pub(crate) fn path_text(path: &Path) -> Result<String, StorageError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(StorageError::Serialization)
}

pub(crate) fn stored_path(value: String) -> PathBuf {
    PathBuf::from(value)
}

pub(crate) fn transport_text(value: TransportKind) -> &'static str {
    match value {
        TransportKind::SystemOpenSsh => "system_open_ssh",
        TransportKind::NativeSsh => "native_ssh",
    }
}

pub(crate) fn transport(value: &str) -> Result<TransportKind, StorageError> {
    match value {
        "system_open_ssh" => Ok(TransportKind::SystemOpenSsh),
        "native_ssh" => Ok(TransportKind::NativeSsh),
        _ => Err(StorageError::Corrupt),
    }
}

pub(crate) fn authentication_text(value: AuthenticationKind) -> &'static str {
    match value {
        AuthenticationKind::Password => "password",
        AuthenticationKind::PublicKey => "public_key",
        AuthenticationKind::Agent => "agent",
        AuthenticationKind::KeyboardInteractive => "keyboard_interactive",
    }
}

pub(crate) fn authentication(value: &str) -> Result<AuthenticationKind, StorageError> {
    match value {
        "password" => Ok(AuthenticationKind::Password),
        "public_key" => Ok(AuthenticationKind::PublicKey),
        "agent" => Ok(AuthenticationKind::Agent),
        "keyboard_interactive" => Ok(AuthenticationKind::KeyboardInteractive),
        _ => Err(StorageError::Corrupt),
    }
}

pub(crate) fn host_key_text(value: HostKeyPolicy) -> &'static str {
    match value {
        HostKeyPolicy::Strict => "strict",
    }
}

pub(crate) fn host_key(value: &str) -> Result<HostKeyPolicy, StorageError> {
    match value {
        "strict" => Ok(HostKeyPolicy::Strict),
        _ => Err(StorageError::Corrupt),
    }
}

pub(crate) fn color_text(value: ColorScheme) -> &'static str {
    match value {
        ColorScheme::Default => "default",
        ColorScheme::OneDark => "one_dark",
        ColorScheme::SolarizedDark => "solarized_dark",
        ColorScheme::SolarizedLight => "solarized_light",
        ColorScheme::Dracula => "dracula",
        ColorScheme::Monokai => "monokai",
        ColorScheme::Nord => "nord",
        ColorScheme::GruvboxDark => "gruvbox_dark",
        ColorScheme::TokyoNight => "tokyo_night",
        ColorScheme::CampbellPowershell => "campbell_powershell",
    }
}

pub(crate) fn color(value: &str) -> Result<ColorScheme, StorageError> {
    match value {
        "default" => Ok(ColorScheme::Default),
        "one_dark" => Ok(ColorScheme::OneDark),
        "solarized_dark" => Ok(ColorScheme::SolarizedDark),
        "solarized_light" => Ok(ColorScheme::SolarizedLight),
        "dracula" => Ok(ColorScheme::Dracula),
        "monokai" => Ok(ColorScheme::Monokai),
        "nord" => Ok(ColorScheme::Nord),
        "gruvbox_dark" => Ok(ColorScheme::GruvboxDark),
        "tokyo_night" => Ok(ColorScheme::TokyoNight),
        "campbell_powershell" => Ok(ColorScheme::CampbellPowershell),
        _ => Err(StorageError::Corrupt),
    }
}

#[derive(Serialize, Deserialize)]
struct OverridesDocument {
    version: u8,
    #[serde(flatten)]
    options: TerminalOverrides,
}

pub(crate) fn overrides_json(value: &TerminalOverrides) -> Result<String, StorageError> {
    serde_json::to_string(&OverridesDocument {
        version: 1,
        options: value.clone(),
    })
    .map_err(|_| StorageError::Serialization)
}

pub(crate) fn overrides(value: &str) -> Result<TerminalOverrides, StorageError> {
    let document: OverridesDocument =
        serde_json::from_str(value).map_err(|_| StorageError::Corrupt)?;
    if document.version != 1 {
        return Err(StorageError::Corrupt);
    }
    Ok(document.options)
}

#[derive(Serialize, Deserialize)]
struct KeyBindingsDocument {
    version: u8,
    key_bindings: Vec<KeyBinding>,
}

pub(crate) fn key_bindings_json(value: &[KeyBinding]) -> Result<String, StorageError> {
    serde_json::to_string(&KeyBindingsDocument {
        version: 1,
        key_bindings: value.to_vec(),
    })
    .map_err(|_| StorageError::Serialization)
}

pub(crate) fn app_settings(
    default_profile: &str,
    color_scheme: &str,
    key_bindings: &str,
) -> Result<AppSettings, StorageError> {
    let document: KeyBindingsDocument =
        serde_json::from_str(key_bindings).map_err(|_| StorageError::Corrupt)?;
    if document.version != 1 {
        return Err(StorageError::Corrupt);
    }
    Ok(AppSettings {
        default_terminal_profile: profile_id(default_profile)?,
        color_scheme: color(color_scheme)?,
        key_bindings: document.key_bindings,
    })
}
