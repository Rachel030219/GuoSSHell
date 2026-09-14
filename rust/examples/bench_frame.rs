//! 回答「整帧传 `RenderFrame` 会不会有性能瓶颈」——用数字回答，不用感觉。
//!
//! 量四件事，同一块屏幕、四种编码：
//!   1. `engine.render()` 本身要多久（这是无论如何都要付的）
//!   2. `RenderFrame` 经 serde_json / bincode（rinf 默认格式）过边界是多少字节、多久
//!   3. 把相邻同属性单元格合并成「run」之后再打包，是多少字节、多久
//!   4. 只发脏行（增量）是多少字节
//!
//! 另外统计每屏的 cell 数与 run 数——后者才是 Dart 侧真正要建的对象数。
//!
//! ```text
//! cargo run --release --example bench_frame
//! ```

use std::time::{Duration, Instant};

use rshell_core::{
    CellAttributes, Color, RenderCell, RenderFrame, RenderRow, ResolvedTerminalProfile,
    TerminalOverrides, TerminalSettingsV1, TerminalSize, Viewport,
};
use rshell_session::{DefaultTerminalEngine, TerminalEngine};

const ITERATIONS: u32 = 200;

/// 一屏终端内容 + 它的名字。
struct Workload {
    name: &'static str,
    note: &'static str,
    bytes: Vec<u8>,
}

/// `ls --color=always -l` 风格：每行若干短 run，颜色密集但每行大部分是空白。
fn workload_ls(cols: u16, rows: u16) -> Vec<u8> {
    let mut out = b"\x1b[H\x1b[2J".to_vec();
    for index in 0..usize::from(rows) {
        out.extend_from_slice(
            format!("\x1b[{};1Hdrwxr-xr-x 12 user staff 384 Sep 14 16:2{} ", index + 1, index % 10)
                .as_bytes(),
        );
        out.extend_from_slice(b"\x1b[1;34m");
        out.extend_from_slice(format!("project_directory_{index}").as_bytes());
        out.extend_from_slice(b"\x1b[0m  \x1b[1;32m");
        out.extend_from_slice(format!("script_{index}.sh").as_bytes());
        out.extend_from_slice(b"\x1b[0m");
        let _ = cols;
    }
    out
}

/// `git diff` 风格：绝大多数单元格是默认色，只有少量 +/- 行着色。
fn workload_diff(cols: u16, rows: u16) -> Vec<u8> {
    let mut out = b"\x1b[H\x1b[2J".to_vec();
    let mut line = 0usize;
    for hunk in 0..(usize::from(rows) / 6 + 1) {
        for text in [
            format!("\x1b[1mdiff --git a/src/module_{hunk}.rs b/src/module_{hunk}.rs\x1b[0m"),
            "\x1b[36m@@ -14,7 +14,9 @@ impl Widget\x1b[0m".to_owned(),
            "     pub fn layout(&self) -> Size {".to_owned(),
            "\x1b[31m-        Size::new(0.0, 0.0)\x1b[0m".to_owned(),
            "\x1b[32m+        let measured = self.measure();\x1b[0m".to_owned(),
            "\x1b[32m+        Size::new(measured.0, measured.1)\x1b[0m".to_owned(),
        ] {
            line += 1;
            if line > usize::from(rows) {
                break;
            }
            out.extend_from_slice(format!("\x1b[{};1H{text}", line).as_bytes());
        }
    }
    let _ = cols;
    out
}

/// 全屏 TUI 重绘（htop 风格）：**每一个**单元格都带独立背景色。
/// 这是结构化帧最坏的情况。
fn workload_tui(cols: u16, rows: u16) -> Vec<u8> {
    let mut out = b"\x1b[H\x1b[2J".to_vec();
    for row in 0..usize::from(rows) {
        out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        let mut buffer = String::new();
        for column in 0..usize::from(cols) {
            buffer.push_str(&format!("\x1b[48;5;{}m ", (row * 7 + column * 3) % 256));
        }
        buffer.push_str("\x1b[0m");
        out.extend_from_slice(buffer.as_bytes());
    }
    out
}

/// CJK 密集：验证宽字符对 run 数和编码字节数的影响。
fn workload_cjk(cols: u16, rows: u16) -> Vec<u8> {
    let mut out = b"\x1b[H\x1b[2J".to_vec();
    for index in 0..usize::from(rows) {
        out.extend_from_slice(
            format!(
                "\x1b[{};1H\x1b[33m构建\x1b[0m 第 {index} 步：编译 Rust 内核并链接 iOS 静态库，耗时 {index}00 毫秒",
                index + 1
            )
            .as_bytes(),
        );
    }
    let _ = cols;
    out
}

fn engine_for(size: TerminalSize) -> DefaultTerminalEngine {
    let profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    DefaultTerminalEngine::new(&profile, size).expect("engine")
}

fn viewport_for(size: TerminalSize) -> Viewport {
    Viewport {
        top_stable_row: i64::MAX,
        rows: size.rows,
    }
}

// ── 编码器 ───────────────────────────────────────────────────────────────────

/// 相邻单元格在 (fg, bg, attrs, selected) 上完全相同才属于同一个 run。
fn same_run(left: &RenderCell, right: &RenderCell) -> bool {
    left.foreground == right.foreground
        && left.background == right.background
        && left.attributes == right.attributes
        && left.selected == right.selected
}

fn run_count(row: &RenderRow) -> usize {
    let cells = &row.cells;
    if cells.is_empty() {
        return 0;
    }
    let mut runs = 1;
    for pair in cells.windows(2) {
        if !same_run(&pair[0], &pair[1]) {
            runs += 1;
        }
    }
    runs
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

/// 把一帧压成「按行 run」的紧凑字节流。空白区域自然合并成一个 run。
fn pack_runs(frame: &RenderFrame, only_dirty_rows: Option<usize>) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    let rows = match only_dirty_rows {
        Some(count) => &frame.rows[..count.min(frame.rows.len())],
        None => &frame.rows[..],
    };
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
        out.extend_from_slice(&(row.stable_row as i32).to_le_bytes());
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

// ── 测量 ─────────────────────────────────────────────────────────────────────

fn measure<F: FnMut() -> usize>(mut body: F) -> (Duration, usize) {
    // 预热一次，避免把首次分配的抖动算进去。
    let first = body();
    let start = Instant::now();
    let mut last = first;
    for _ in 0..ITERATIONS {
        last = body();
    }
    (start.elapsed() / ITERATIONS, last)
}

fn mean_micros<F: FnMut()>(mut body: F) -> f64 {
    body();
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        body();
    }
    start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(ITERATIONS)
}

fn main() {
    let sizes = [
        ("iPad 横屏 120x40", TerminalSize { cols: 120, rows: 40, pixel_width: 0, pixel_height: 0, dpi: 264 }),
        ("iPhone 竖屏 40x22", TerminalSize { cols: 40, rows: 22, pixel_width: 0, pixel_height: 0, dpi: 460 }),
    ];

    for (label, size) in sizes {
        let cells_total = usize::from(size.cols) * usize::from(size.rows);
        println!("\n══════════════════════════════════════════════════════════");
        println!("{label}   （{cells_total} cells/帧，主循环预算 16.67 ms）");
        println!("══════════════════════════════════════════════════════════");
        println!(
            "{:<12} {:>9} {:>7} {:>7} {:>10} {:>10} {:>7} {:>7} {:>8}",
            "负载", "render", "run/行", "run总", "serde_json", "bincode", "runs", "脏3行", "非空cell"
        );

        let workloads = [
            Workload { name: "ls --color", note: "短 run 密集", bytes: workload_ls(size.cols, size.rows) },
            Workload { name: "git diff", note: "大面积为默认色", bytes: workload_diff(size.cols, size.rows) },
            Workload { name: "TUI 满屏", note: "每 cell 独立底色（最坏）", bytes: workload_tui(size.cols, size.rows) },
            Workload { name: "CJK 日志", note: "宽字符", bytes: workload_cjk(size.cols, size.rows) },
        ];

        for workload in &workloads {
            let mut engine = engine_for(size);
            engine.advance(&workload.bytes).expect("advance");
            let viewport = viewport_for(size);

            // render() 本身的开销：无论怎么编码都要付。
            let frame = engine.render(viewport, None).expect("render");
            let render_micros = mean_micros(|| {
                let _ = engine.render(viewport, None).expect("render");
            });

            // 这一帧的形态统计。
            let row_count = frame.rows.len().max(1);
            let runs_total: usize = frame.rows.iter().map(run_count).sum();
            let non_blank: usize = frame
                .rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .filter(|cell| cell.text != " " || cell.background != Color::Default)
                        .count()
                })
                .sum();

            let json_bytes = serde_json::to_vec(&*frame).expect("json").len();
            let (bincode_duration, bincode_bytes) = measure(|| {
                bincode::serde::encode_to_vec(&*frame, bincode::config::standard())
                    .expect("bincode")
                    .len()
            });
            let packed = pack_runs(&frame, None);
            let dirty = pack_runs(&frame, Some(3));

            println!(
                "{:<12} {:>7.2}ms {:>7.1} {:>7} {:>9.1}KB {:>8.1}KB {:>6.1}KB {:>6.1}KB {:>8}",
                workload.name,
                render_micros / 1000.0,
                runs_total as f64 / row_count as f64,
                runs_total,
                json_bytes as f64 / 1024.0,
                bincode_bytes as f64 / 1024.0,
                packed.len() as f64 / 1024.0,
                dirty.len() as f64 / 1024.0,
                non_blank,
            );
            let _ = bincode_duration;

            // 单独量一次打包耗时（release 下这块应当远小于 render）。
            let pack_micros = mean_micros(|| {
                let _ = pack_runs(&frame, None);
            });
            let bincode_micros = mean_micros(|| {
                let _ = bincode::serde::encode_to_vec(&*frame, bincode::config::standard());
            });
            println!(
                "             └─ 编码耗时：bincode {:.2}ms · run 打包 {:.2}ms · 备注：{}",
                bincode_micros / 1000.0,
                pack_micros / 1000.0,
                workload.note
            );
        }
    }

    println!("\n注：`render()` 会为每个 cell 分配一个 String（`RenderCell.text: String`），");
    println!("    4800 cells 即 4800 次堆分配/帧。上面的 render 耗时里包含这部分。");
}
