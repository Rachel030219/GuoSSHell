use serde::{Deserialize, Serialize, de::Error as _};

use crate::connection::TerminalProfileId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    Default,
    OneDark,
    SolarizedDark,
    SolarizedLight,
    Dracula,
    Monokai,
    Nord,
    GruvboxDark,
    TokyoNight,
    CampbellPowershell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
    Character(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSettingsVersion(());

impl TerminalSettingsVersion {
    pub const V1: Self = Self(());
}

impl Default for TerminalSettingsVersion {
    fn default() -> Self {
        Self::V1
    }
}

impl Serialize for TerminalSettingsVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for TerminalSettingsVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            1 => Ok(Self::V1),
            version => Err(D::Error::custom(format!(
                "unsupported terminal settings version {version}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalSettingsV1 {
    pub version: TerminalSettingsVersion,
    pub terminal_type: String,
    pub initial_cols: u16,
    pub initial_rows: u16,
    pub scrollback_lines: usize,
    pub font_family: String,
    pub font_size: f32,
    pub color_scheme: ColorScheme,
    pub key_bindings: Vec<KeyBinding>,
    pub left_alt_as_meta: bool,
    pub right_alt_as_meta: bool,
    pub enable_csi_u: bool,
    pub enable_kitty_keyboard: bool,
    pub mouse_reporting: bool,
    pub scroll_on_output: bool,
    pub scroll_on_keypress: bool,
    pub answerback: String,
}

impl Default for TerminalSettingsV1 {
    fn default() -> Self {
        Self {
            version: TerminalSettingsVersion::V1,
            terminal_type: "xterm-256color".into(),
            initial_cols: 120,
            initial_rows: 36,
            scrollback_lines: 6_000,
            font_family: "CaskaydiaCove NF Mono".into(),
            font_size: 18.0,
            color_scheme: ColorScheme::default(),
            key_bindings: Vec::new(),
            left_alt_as_meta: true,
            right_alt_as_meta: true,
            enable_csi_u: false,
            enable_kitty_keyboard: false,
            mouse_reporting: true,
            scroll_on_output: true,
            scroll_on_keypress: false,
            answerback: "rsHell".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalProfile {
    pub id: TerminalProfileId,
    pub name: String,
    pub settings: TerminalSettingsV1,
}

impl TerminalProfile {
    pub fn p0_default() -> Self {
        Self {
            id: TerminalProfileId(uuid::Uuid::from_u128(1)),
            name: "Default".into(),
            settings: TerminalSettingsV1::default(),
        }
    }
}

impl Default for TerminalProfile {
    fn default() -> Self {
        Self::p0_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    pub default_terminal_profile: TerminalProfileId,
    pub color_scheme: ColorScheme,
    pub key_bindings: Vec<KeyBinding>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_terminal_profile: TerminalProfile::p0_default().id,
            color_scheme: ColorScheme::default(),
            key_bindings: Vec::new(),
        }
    }
}
