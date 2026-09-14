use rshell_core::{ColorScheme, KeyBinding, KeyCode, KeyModifiers, TerminalOverrides};

use super::{ImportError, ImportWarning, legacy::LegacyTerminal, legacy_mapping::push_warning};

pub(super) fn map_terminal(
    terminal: LegacyTerminal,
    warnings: &mut Vec<ImportWarning>,
) -> Result<TerminalOverrides, ImportError> {
    if terminal.enable_kitty_graphics == Some(true) {
        push_warning(warnings, ImportWarning::KittyGraphicsDisabled);
    }
    Ok(TerminalOverrides {
        terminal_type: optional_text(terminal.terminal_type),
        initial_cols: terminal.initial_cols,
        initial_rows: terminal.initial_rows,
        scrollback_lines: terminal.scrollback_lines,
        font_family: None,
        font_size: terminal.font_size.map(f32::from),
        color_scheme: terminal
            .color_scheme
            .map(|value| color_scheme(&value))
            .transpose()?,
        key_bindings: key_bindings(terminal.delete_key, terminal.backspace_key)?,
        left_alt_as_meta: terminal.left_alt_as_meta,
        right_alt_as_meta: terminal.right_alt_as_meta,
        enable_csi_u: terminal.enable_csi_u,
        enable_kitty_keyboard: terminal.enable_kitty_keyboard,
        mouse_reporting: terminal.mouse_reporting,
        scroll_on_output: terminal.scroll_on_output,
        scroll_on_keypress: terminal.scroll_on_keypress,
        answerback: optional_text(terminal.answerback),
    })
}

fn color_scheme(value: &str) -> Result<ColorScheme, ImportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "default" => Ok(ColorScheme::Default),
        "one_dark" => Ok(ColorScheme::OneDark),
        "solarized_dark" => Ok(ColorScheme::SolarizedDark),
        "solarized_light" => Ok(ColorScheme::SolarizedLight),
        "dracula" => Ok(ColorScheme::Dracula),
        "monokai" => Ok(ColorScheme::Monokai),
        "nord" => Ok(ColorScheme::Nord),
        "gruvbox_dark" => Ok(ColorScheme::GruvboxDark),
        "tokyo_night" => Ok(ColorScheme::TokyoNight),
        "campbell_powershell" => Ok(ColorScheme::CampbellPowershell),
        _ => Err(ImportError::InvalidConnection),
    }
}

fn key_bindings(
    delete: Option<String>,
    backspace: Option<String>,
) -> Result<Option<Vec<KeyBinding>>, ImportError> {
    let mut bindings = Vec::new();
    if let Some(value) = delete.and_then(nonempty) {
        bindings.push(binding(KeyCode::Delete, delete_sequence(&value)?));
    }
    if let Some(value) = backspace.and_then(nonempty) {
        bindings.push(binding(KeyCode::Backspace, backspace_sequence(&value)?));
    }
    Ok((!bindings.is_empty()).then_some(bindings))
}

fn binding(code: KeyCode, sequence: &str) -> KeyBinding {
    KeyBinding {
        code,
        modifiers: KeyModifiers::default(),
        action: format!("send:{sequence}"),
    }
}

fn delete_sequence(value: &str) -> Result<&'static str, ImportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "vt220_del" => Ok("\u{1b}[3~"),
        "ascii127" => Ok("\u{7f}"),
        "backspace" => Ok("\u{8}"),
        _ => Err(ImportError::InvalidConnection),
    }
}

fn backspace_sequence(value: &str) -> Result<&'static str, ImportError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "vt220_del" => Ok("\u{1b}[3~"),
        "ascii127" => Ok("\u{7f}"),
        "ctrl_h" => Ok("\u{8}"),
        _ => Err(ImportError::InvalidConnection),
    }
}

fn optional_text(value: Option<String>) -> Option<String> {
    value.and_then(nonempty)
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
