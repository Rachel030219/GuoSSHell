//! `RenderFrame` → 紧凑 run 字节流（run 压缩 + 打包）。
//!
//! 编码器从 `rust/examples/bench_frame.rs` **原样提升**（不是重写）——
//! 那是 §4 实测过 4,800 格 → 73–200 run → 5.8–7.2 KB/帧 的同一套代码。
//! 相对 bench 的唯一调整：`stable_row` 保持上游的 `i64` 原宽，不再截成 `i32`。
//!
//! 脏行增量尚未实现：`TerminalEngine::advance` 只给帧级 `dirty: bool`，
//! 行级脏标记上游没有暴露；M1 先发整帧压缩（典型 5.8–7.2 KB，TUI 满屏最坏 61 KB），
//! 行级增量等上游/适配层有脏行信息后再加。

use rshell_core::{CellAttributes, Color, RenderCell, RenderFrame};

/// wire 格式（全部**小端**）：
///
/// ```text
/// row_count: u16
/// 每行:
///   run_count: u16 · stable_row: i64 · wrapped: u8
///   每个 run:
///     start: u16 · len: u16 · fg · bg · attrs: u8 · text_len: u32 · text: utf8
/// ```
///
/// `start` / `len` 以**列**计（CJK 占 2 列，text 的字符数 ≤ len）。
/// `fg` / `bg`：`0` = Default；`1` + `u8` = Ansi(index)；`2` + r,g,b = Rgb。
/// `attrs` 位：0 bold · 1 italic · 2 underline · 3 strike · 4 reverse · 5 selected。
pub fn pack_runs(frame: &RenderFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    let rows = &frame.rows[..];
    out.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    for row in rows {
        let mut runs: Vec<(u16, u16, &RenderCell, String)> = Vec::new();
        for (index, cell) in row.cells.iter().enumerate() {
            match runs.last_mut() {
                Some((_, len, head, text)) if same_run(head, cell) => {
                    *len += 1;
                    text.push_str(&cell.text);
                }
                _ => runs.push((index as u16, 1, cell, cell.text.clone())),
            }
        }
        out.extend_from_slice(&(runs.len() as u16).to_le_bytes());
        out.extend_from_slice(&row.stable_row.to_le_bytes());
        out.push(u8::from(row.wrapped));
        for (start, len, cell, text) in runs {
            out.extend_from_slice(&start.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            write_color(cell.foreground, &mut out);
            write_color(cell.background, &mut out);
            write_attrs(cell.attributes, cell.selected, &mut out);
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
    }
    out
}

/// 相邻单元格在 (fg, bg, attrs, selected) 上完全相同才属于同一个 run。
fn same_run(left: &RenderCell, right: &RenderCell) -> bool {
    left.foreground == right.foreground
        && left.background == right.background
        && left.attributes == right.attributes
        && left.selected == right.selected
}

fn write_color(color: Color, out: &mut Vec<u8>) {
    match color {
        Color::Default => out.push(0),
        Color::Ansi(index) => {
            out.push(1);
            out.push(index);
        }
        Color::Rgb(r, g, b) => {
            out.push(2);
            out.extend_from_slice(&[r, g, b]);
        }
    }
}

fn write_attrs(attributes: CellAttributes, selected: bool, out: &mut Vec<u8>) {
    let mut bits = 0u8;
    bits |= u8::from(attributes.bold);
    bits |= u8::from(attributes.italic) << 1;
    bits |= u8::from(attributes.underline) << 2;
    bits |= u8::from(attributes.strike) << 3;
    bits |= u8::from(attributes.reverse) << 4;
    bits |= u8::from(selected) << 5;
    out.push(bits);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::pack_runs;
    use rshell_core::{
        ResolvedTerminalProfile, TerminalOverrides, TerminalSettingsV1, TerminalSize, Viewport,
    };
    use rshell_session::{DefaultTerminalEngine, TerminalEngine};

    fn size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        }
    }

    #[test]
    fn packed_frame_matches_wire_format() {
        let profile: ResolvedTerminalProfile =
            TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let mut engine = DefaultTerminalEngine::new(&profile, size()).expect("engine");
        engine
            .advance(b"\x1b[31mred\x1b[0m plain\r\n\xe4\xb8\xad\xe6\x96\x87 wide\r\n")
            .expect("advance");

        let frame = engine
            .render(
                Viewport {
                    top_stable_row: i64::MAX,
                    rows: 24,
                },
                None,
            )
            .expect("render");

        let packed = pack_runs(&frame);
        let mut cursor = 0usize;
        let read_u16 = |cursor: &mut usize| {
            let value = u16::from_le_bytes([packed[*cursor], packed[*cursor + 1]]);
            *cursor += 2;
            value
        };

        let row_count = read_u16(&mut cursor) as usize;
        assert_eq!(row_count, frame.rows.len(), "row count must be header");
        assert_eq!(row_count, 24);

        let mut saw_red = false;
        for row_index in 0..row_count {
            let run_count = read_u16(&mut cursor) as usize;
            let _stable_row = {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&packed[cursor..cursor + 8]);
                cursor += 8;
                i64::from_le_bytes(bytes)
            };
            let _wrapped = packed[cursor];
            cursor += 1;

            for _ in 0..run_count {
                let _start = read_u16(&mut cursor);
                let _len = read_u16(&mut cursor);
                // fg：red 应该以 Ansi(1)（SGR 31 → 前景红）出现在第 0 行。
                match packed[cursor] {
                    1 => {
                        if row_index == 0 && packed[cursor + 1] == 1 {
                            saw_red = true;
                        }
                        cursor += 2;
                    }
                    2 => cursor += 4,
                    _ => cursor += 1,
                }
                // bg
                match packed[cursor] {
                    1 => cursor += 2,
                    2 => cursor += 4,
                    _ => cursor += 1,
                }
                cursor += 1; // attrs
                let text_len = u32::from_le_bytes([
                    packed[cursor],
                    packed[cursor + 1],
                    packed[cursor + 2],
                    packed[cursor + 3],
                ]) as usize;
                cursor += 4;
                cursor += text_len;
            }
        }
        assert!(saw_red, "red run (Ansi(1)) must appear on row 0");
        assert_eq!(
            cursor,
            packed.len(),
            "decoder must consume exactly all bytes"
        );
    }
}
