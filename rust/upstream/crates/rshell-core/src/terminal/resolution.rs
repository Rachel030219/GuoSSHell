use serde::{Deserialize, Serialize};

use super::{ColorScheme, KeyBinding, TerminalSettingsV1};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_rows: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<ColorScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_bindings: Option<Vec<KeyBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_alt_as_meta: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_alt_as_meta: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_csi_u: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_kitty_keyboard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_reporting: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_on_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_on_keypress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answerback: Option<String>,
}

impl TerminalOverrides {
    pub const FIELD_COUNT: usize = 16;

    pub fn explicit_field_count(&self) -> usize {
        [
            self.terminal_type.is_some(),
            self.initial_cols.is_some(),
            self.initial_rows.is_some(),
            self.scrollback_lines.is_some(),
            self.font_family.is_some(),
            self.font_size.is_some(),
            self.color_scheme.is_some(),
            self.key_bindings.is_some(),
            self.left_alt_as_meta.is_some(),
            self.right_alt_as_meta.is_some(),
            self.enable_csi_u.is_some(),
            self.enable_kitty_keyboard.is_some(),
            self.mouse_reporting.is_some(),
            self.scroll_on_output.is_some(),
            self.scroll_on_keypress.is_some(),
            self.answerback.is_some(),
        ]
        .into_iter()
        .filter(|explicit| *explicit)
        .count()
    }

    pub fn inherited_field_count(&self) -> usize {
        Self::FIELD_COUNT - self.explicit_field_count()
    }

    #[must_use]
    pub fn clear_all(&self) -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTerminalProfile {
    pub terminal_type: String,
    pub cols: u16,
    pub rows: u16,
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

impl TerminalSettingsV1 {
    pub fn resolve(&self, overrides: &TerminalOverrides) -> ResolvedTerminalProfile {
        let defaults = Self::default();
        ResolvedTerminalProfile {
            terminal_type: text(
                overrides
                    .terminal_type
                    .as_ref()
                    .unwrap_or(&self.terminal_type),
                &defaults.terminal_type,
            ),
            cols: overrides
                .initial_cols
                .unwrap_or(self.initial_cols)
                .clamp(1, 999),
            rows: overrides
                .initial_rows
                .unwrap_or(self.initial_rows)
                .clamp(1, 999),
            scrollback_lines: overrides
                .scrollback_lines
                .unwrap_or(self.scrollback_lines)
                .clamp(100, 1_000_000),
            font_family: text(
                overrides.font_family.as_ref().unwrap_or(&self.font_family),
                &defaults.font_family,
            ),
            font_size: finite_clamp(
                overrides.font_size.unwrap_or(self.font_size),
                6.0,
                72.0,
                defaults.font_size,
            ),
            color_scheme: overrides.color_scheme.unwrap_or(self.color_scheme),
            key_bindings: overrides
                .key_bindings
                .clone()
                .unwrap_or_else(|| self.key_bindings.clone()),
            left_alt_as_meta: overrides.left_alt_as_meta.unwrap_or(self.left_alt_as_meta),
            right_alt_as_meta: overrides
                .right_alt_as_meta
                .unwrap_or(self.right_alt_as_meta),
            enable_csi_u: overrides.enable_csi_u.unwrap_or(self.enable_csi_u),
            enable_kitty_keyboard: overrides
                .enable_kitty_keyboard
                .unwrap_or(self.enable_kitty_keyboard),
            mouse_reporting: overrides.mouse_reporting.unwrap_or(self.mouse_reporting),
            scroll_on_output: overrides.scroll_on_output.unwrap_or(self.scroll_on_output),
            scroll_on_keypress: overrides
                .scroll_on_keypress
                .unwrap_or(self.scroll_on_keypress),
            answerback: text(
                overrides.answerback.as_ref().unwrap_or(&self.answerback),
                &defaults.answerback,
            ),
        }
    }
}

impl ResolvedTerminalProfile {
    pub fn with_app_key_bindings(mut self, app_bindings: &[KeyBinding]) -> Self {
        for app_binding in app_bindings {
            let shadowed = self.key_bindings.iter().any(|profile_binding| {
                profile_binding.code == app_binding.code
                    && profile_binding.modifiers == app_binding.modifiers
            });
            if !shadowed {
                self.key_bindings.push(app_binding.clone());
            }
        }
        self
    }
}

fn text(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.into()
    } else {
        trimmed.into()
    }
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}
