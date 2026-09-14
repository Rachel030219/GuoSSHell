use alacritty_terminal::term::TermMode;
use rshell_core::{MouseButton, MouseEventKind, TerminalMouseEvent};

use crate::EngineError;

pub(crate) fn encode(event: TerminalMouseEvent, mode: TermMode) -> Result<Vec<u8>, EngineError> {
    validate(event)?;
    if !reports(event, mode) {
        return Err(EngineError::UnsupportedMouse("mouse event is not tracked"));
    }

    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut button = button_code(event.kind, event.button, sgr)?;
    button += modifier_bits(event);
    if event.kind == MouseEventKind::Move {
        button += 32;
    }
    let x = u32::from(event.cell.column) + 1;
    let y = u32::from(event.viewport_row) + 1;
    if sgr {
        let suffix = if event.kind == MouseEventKind::Release {
            'm'
        } else {
            'M'
        };
        return Ok(format!("\x1b[<{button};{x};{y}{suffix}").into());
    }

    let mut encoded = b"\x1b[M".to_vec();
    push_coordinate(&mut encoded, u32::from(button) + 32, mode);
    push_coordinate(&mut encoded, x + 32, mode);
    push_coordinate(&mut encoded, y + 32, mode);
    Ok(encoded)
}

fn reports(event: TerminalMouseEvent, mode: TermMode) -> bool {
    match event.kind {
        MouseEventKind::Press | MouseEventKind::Release | MouseEventKind::Scroll => {
            mode.intersects(TermMode::MOUSE_MODE)
        }
        MouseEventKind::Move if event.button.is_some() => {
            mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        }
        MouseEventKind::Move => mode.contains(TermMode::MOUSE_MOTION),
    }
}

fn validate(event: TerminalMouseEvent) -> Result<(), EngineError> {
    match (event.kind, event.button) {
        (MouseEventKind::Press, None) => Err(EngineError::UnsupportedMouse("press has no button")),
        (MouseEventKind::Scroll, Some(MouseButton::WheelUp | MouseButton::WheelDown)) => Ok(()),
        (MouseEventKind::Scroll, _) => Err(EngineError::UnsupportedMouse(
            "scroll has no wheel direction",
        )),
        (_, Some(MouseButton::Back | MouseButton::Forward)) => Err(EngineError::UnsupportedMouse(
            "back and forward mouse buttons are unsupported",
        )),
        (_, Some(MouseButton::WheelUp | MouseButton::WheelDown)) => {
            Err(EngineError::UnsupportedMouse("wheel used outside scroll"))
        }
        _ => Ok(()),
    }
}

fn button_code(
    kind: MouseEventKind,
    button: Option<MouseButton>,
    sgr: bool,
) -> Result<u8, EngineError> {
    if kind == MouseEventKind::Release && !sgr {
        return Ok(3);
    }
    Ok(match button {
        Some(MouseButton::Left) => 0,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        Some(MouseButton::WheelUp) => 64,
        Some(MouseButton::WheelDown) => 65,
        None => 3,
        Some(MouseButton::Back | MouseButton::Forward) => {
            return Err(EngineError::UnsupportedMouse("unsupported mouse button"));
        }
    })
}

fn modifier_bits(event: TerminalMouseEvent) -> u8 {
    4 * u8::from(event.modifiers.shift)
        + 8 * u8::from(event.modifiers.alt)
        + 16 * u8::from(event.modifiers.control)
}

fn push_coordinate(output: &mut Vec<u8>, value: u32, mode: TermMode) {
    if mode.contains(TermMode::UTF8_MOUSE) {
        let character = char::from_u32(value.min(0x7ff)).unwrap_or('\u{7ff}');
        let mut bytes = [0; 4];
        output.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
    } else {
        output.push(value.min(u32::from(u8::MAX)) as u8);
    }
}
