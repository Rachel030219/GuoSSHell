use std::collections::HashSet;

use super::{
    AppSettings, KeyBinding, KeyCode, TerminalOverrides, TerminalProfile, TerminalSettingsV1,
    parse_terminal_key_action,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsValidationCode {
    Blank,
    OutOfRange,
    NonFinite,
    InvalidChord,
    DuplicateBinding,
    InvalidAction,
    UnknownProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsValidationError {
    pub field: &'static str,
    pub code: SettingsValidationCode,
}

impl std::fmt::Display for SettingsValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid terminal setting: {}", self.field)
    }
}

impl std::error::Error for SettingsValidationError {}

pub fn validate_terminal_profile(profile: &TerminalProfile) -> Result<(), SettingsValidationError> {
    nonblank("profile.name", &profile.name)?;
    validate_terminal_settings(&profile.settings)
}

pub fn validate_terminal_settings(
    settings: &TerminalSettingsV1,
) -> Result<(), SettingsValidationError> {
    nonblank("terminal_type", &settings.terminal_type)?;
    range("initial_cols", settings.initial_cols as usize, 1, 999)?;
    range("initial_rows", settings.initial_rows as usize, 1, 999)?;
    range(
        "scrollback_lines",
        settings.scrollback_lines,
        100,
        1_000_000,
    )?;
    nonblank("font_family", &settings.font_family)?;
    if !settings.font_size.is_finite() {
        return Err(error("font_size", SettingsValidationCode::NonFinite));
    }
    if !(6.0..=72.0).contains(&settings.font_size) {
        return Err(error("font_size", SettingsValidationCode::OutOfRange));
    }
    nonblank("answerback", &settings.answerback)?;
    validate_bindings(&settings.key_bindings)
}

pub fn validate_terminal_overrides(
    overrides: &TerminalOverrides,
) -> Result<(), SettingsValidationError> {
    if let Some(value) = &overrides.terminal_type {
        nonblank("terminal_overrides.terminal_type", value)?;
    }
    if let Some(value) = overrides.initial_cols {
        range("terminal_overrides.initial_cols", value as usize, 1, 999)?;
    }
    if let Some(value) = overrides.initial_rows {
        range("terminal_overrides.initial_rows", value as usize, 1, 999)?;
    }
    if let Some(value) = overrides.scrollback_lines {
        range("terminal_overrides.scrollback_lines", value, 100, 1_000_000)?;
    }
    if let Some(value) = &overrides.font_family {
        nonblank("terminal_overrides.font_family", value)?;
    }
    if let Some(value) = overrides.font_size {
        if !value.is_finite() {
            return Err(error(
                "terminal_overrides.font_size",
                SettingsValidationCode::NonFinite,
            ));
        }
        if !(6.0..=72.0).contains(&value) {
            return Err(error(
                "terminal_overrides.font_size",
                SettingsValidationCode::OutOfRange,
            ));
        }
    }
    if let Some(bindings) = &overrides.key_bindings {
        validate_bindings(bindings)?;
    }
    if let Some(value) = &overrides.answerback {
        nonblank("terminal_overrides.answerback", value)?;
    }
    Ok(())
}

pub fn validate_app_settings(
    settings: &AppSettings,
    profiles: &[TerminalProfile],
) -> Result<(), SettingsValidationError> {
    if !profiles
        .iter()
        .any(|profile| profile.id == settings.default_terminal_profile)
    {
        return Err(error(
            "default_terminal_profile",
            SettingsValidationCode::UnknownProfile,
        ));
    }
    validate_bindings(&settings.key_bindings)
}

fn validate_bindings(bindings: &[KeyBinding]) -> Result<(), SettingsValidationError> {
    let mut chords = HashSet::new();
    for binding in bindings {
        let code = chord_code(&binding.code)?;
        parse_terminal_key_action(&binding.action)?;
        let chord = (
            code,
            binding.modifiers.shift,
            binding.modifiers.control,
            binding.modifiers.alt,
            binding.modifiers.super_key,
        );
        if !chords.insert(chord) {
            return Err(error(
                "key_bindings",
                SettingsValidationCode::DuplicateBinding,
            ));
        }
    }
    Ok(())
}

fn chord_code(code: &KeyCode) -> Result<String, SettingsValidationError> {
    match code {
        KeyCode::Character(character) if character.is_control() => Err(error(
            "key_bindings.code",
            SettingsValidationCode::InvalidChord,
        )),
        KeyCode::F(number) if !(1..=24).contains(number) => Err(error(
            "key_bindings.code",
            SettingsValidationCode::InvalidChord,
        )),
        _ => Ok(format!("{code:?}")),
    }
}

fn nonblank(field: &'static str, value: &str) -> Result<(), SettingsValidationError> {
    if value.trim().is_empty() {
        Err(error(field, SettingsValidationCode::Blank))
    } else {
        Ok(())
    }
}

fn range(
    field: &'static str,
    value: usize,
    min: usize,
    max: usize,
) -> Result<(), SettingsValidationError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(error(field, SettingsValidationCode::OutOfRange))
    }
}

const fn error(field: &'static str, code: SettingsValidationCode) -> SettingsValidationError {
    SettingsValidationError { field, code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyModifiers, TerminalSettingsV1};

    #[test]
    fn rejects_duplicate_and_invalid_chords_without_normalizing_them() {
        let binding = KeyBinding {
            code: KeyCode::Character('t'),
            modifiers: KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
            action: "new_tab".into(),
        };
        let mut settings = TerminalSettingsV1 {
            key_bindings: vec![binding.clone(), binding],
            ..TerminalSettingsV1::default()
        };
        assert_eq!(
            validate_terminal_settings(&settings).unwrap_err().code,
            SettingsValidationCode::DuplicateBinding
        );
        settings.key_bindings = vec![KeyBinding {
            code: KeyCode::F(0),
            modifiers: KeyModifiers::default(),
            action: "invalid".into(),
        }];
        assert_eq!(
            validate_terminal_settings(&settings).unwrap_err().code,
            SettingsValidationCode::InvalidChord
        );
    }
}
