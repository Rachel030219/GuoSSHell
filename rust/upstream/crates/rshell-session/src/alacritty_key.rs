use alacritty_terminal::term::TermMode;
use rshell_core::{KeyCode, KeyModifiers};

use crate::EngineError;

pub(crate) fn encode(
    code: KeyCode,
    modifiers: KeyModifiers,
    mode: TermMode,
    csi_u: bool,
) -> Result<Vec<u8>, EngineError> {
    if modifiers.super_key {
        return Err(EngineError::UnsupportedInput("super-modified key"));
    }
    let protocol = csi_u || mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    match code {
        KeyCode::Character(character) => encode_character(character, modifiers, protocol),
        KeyCode::Enter => Ok(prefixed(b"\r", modifiers.alt)),
        KeyCode::Escape => Ok(prefixed(b"\x1b", modifiers.alt)),
        KeyCode::Tab => Ok(encode_tab(modifiers, protocol)),
        KeyCode::Backspace => Ok(prefixed(
            if modifiers.control { b"\x08" } else { b"\x7f" },
            modifiers.alt,
        )),
        KeyCode::Delete => Ok(tilde(3, modifiers)),
        KeyCode::Insert => Ok(tilde(2, modifiers)),
        KeyCode::Home => Ok(cursor_key(b'H', modifiers, mode)),
        KeyCode::End => Ok(cursor_key(b'F', modifiers, mode)),
        KeyCode::PageUp => Ok(tilde(5, modifiers)),
        KeyCode::PageDown => Ok(tilde(6, modifiers)),
        KeyCode::ArrowUp => Ok(cursor_key(b'A', modifiers, mode)),
        KeyCode::ArrowDown => Ok(cursor_key(b'B', modifiers, mode)),
        KeyCode::ArrowRight => Ok(cursor_key(b'C', modifiers, mode)),
        KeyCode::ArrowLeft => Ok(cursor_key(b'D', modifiers, mode)),
        KeyCode::F(number @ 1..=24) => Ok(function_key(number, modifiers)),
        KeyCode::F(_) => Err(EngineError::UnsupportedInput("function key outside F1-F24")),
    }
}

fn encode_character(
    character: char,
    modifiers: KeyModifiers,
    protocol: bool,
) -> Result<Vec<u8>, EngineError> {
    if protocol && modifiers.control {
        return Ok(format!(
            "\x1b[{};{}u",
            character as u32,
            modifier_parameter(modifiers)
        )
        .into());
    }

    let mut bytes = Vec::new();
    if modifiers.alt {
        bytes.push(0x1b);
    }
    if modifiers.control {
        let control = control_character(character).ok_or(EngineError::UnsupportedInput(
            "unsupported control character",
        ))?;
        bytes.push(control);
    } else {
        let mut encoded = [0; 4];
        bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
    }
    Ok(bytes)
}

fn encode_tab(modifiers: KeyModifiers, protocol: bool) -> Vec<u8> {
    if modifiers.control && protocol {
        return format!("\x1b[9;{}u", modifier_parameter(modifiers)).into();
    }
    match (modifiers.shift, modifiers.control, modifiers.alt) {
        (false, false, false) => b"\t".to_vec(),
        (true, false, false) => b"\x1b[Z".to_vec(),
        (false, true, false) => b"\x1b[9;5u".to_vec(),
        (true, true, false) => b"\x1b[1;5Z".to_vec(),
        _ => prefixed(b"\t", modifiers.alt),
    }
}

fn cursor_key(final_byte: u8, modifiers: KeyModifiers, mode: TermMode) -> Vec<u8> {
    if no_modifiers(modifiers) {
        let prefix = if mode.contains(TermMode::APP_CURSOR) {
            b"\x1bO".as_slice()
        } else {
            b"\x1b[".as_slice()
        };
        return [prefix, &[final_byte]].concat();
    }
    format!(
        "\x1b[1;{}{}",
        modifier_parameter(modifiers),
        final_byte as char
    )
    .into()
}

fn tilde(number: u8, modifiers: KeyModifiers) -> Vec<u8> {
    if no_modifiers(modifiers) {
        format!("\x1b[{number}~").into()
    } else {
        format!("\x1b[{number};{}~", modifier_parameter(modifiers)).into()
    }
}

fn function_key(number: u8, modifiers: KeyModifiers) -> Vec<u8> {
    if number <= 4 {
        let final_byte = b'P' + number - 1;
        if no_modifiers(modifiers) {
            return vec![0x1b, b'O', final_byte];
        }
        return format!(
            "\x1b[1;{}{}",
            modifier_parameter(modifiers),
            final_byte as char
        )
        .into();
    }
    const CODES: [u8; 20] = [
        15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34, 42, 43, 44, 45,
    ];
    tilde(CODES[usize::from(number - 5)], modifiers)
}

fn modifier_parameter(modifiers: KeyModifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.control)
}

fn no_modifiers(modifiers: KeyModifiers) -> bool {
    !modifiers.shift && !modifiers.control && !modifiers.alt
}

fn prefixed(bytes: &[u8], alt: bool) -> Vec<u8> {
    [if alt { b"\x1b".as_slice() } else { &[] }, bytes].concat()
}

fn control_character(character: char) -> Option<u8> {
    match character {
        ' ' | '@' => Some(0),
        'a'..='z' | 'A'..='Z' => Some((character.to_ascii_uppercase() as u8) & 0x1f),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}
