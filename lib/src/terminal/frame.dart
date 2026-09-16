import 'dart:convert';
import 'dart:typed_data';

/// 终端颜色（边界上的三种形态，对应 `rshell_core::Color`）。
sealed class TermColor {
  const TermColor();
}

class DefaultColor extends TermColor {
  const DefaultColor();
}

class AnsiColor extends TermColor {
  final int index;
  const AnsiColor(this.index);
}

class RgbColor extends TermColor {
  final int r, g, b;
  const RgbColor(this.r, this.g, this.b);
}

/// 属性位（与 frame_codec.rs 的 wire 格式一致）。
class Attr {
  static const int bold = 1;
  static const int italic = 2;
  static const int underline = 4;
  static const int strike = 8;
  static const int reverse = 16;
  static const int selected = 32;
}

/// 一段连续同属性单元格。`start`/`len` 以列计（CJK 占 2 列），
/// `text` 的字符数可以小于 `len`（宽字符）。
class FrameRun {
  final int start, len, attrs;
  final TermColor fg, bg;
  final String text;
  const FrameRun({
    required this.start,
    required this.len,
    required this.attrs,
    required this.fg,
    required this.bg,
    required this.text,
  });

  bool get bold => attrs & Attr.bold != 0;
  bool get italic => attrs & Attr.italic != 0;
  bool get underline => attrs & Attr.underline != 0;
  bool get strike => attrs & Attr.strike != 0;
  bool get reverse => attrs & Attr.reverse != 0;
  bool get selected => attrs & Attr.selected != 0;
}

class FrameRow {
  final int stableRow;
  final bool wrapped;
  final List<FrameRun> runs;
  const FrameRow({
    required this.stableRow,
    required this.wrapped,
    required this.runs,
  });
}

/// 一帧画面：几何（cols/rows/cursor）+ 按行的 run 列表。
/// 这是**纯展示数据**——Dart 不持有终端状态机（PLAN.md 铁律 4），
/// 每一帧都是 Rust 权威状态的完整快照投影。
class TerminalFrame {
  final int cols, rows;
  final int cursorCol, cursorRow;
  final List<FrameRow> lines;
  const TerminalFrame({
    required this.cols,
    required this.rows,
    required this.cursorCol,
    required this.cursorRow,
    required this.lines,
  });
}

/// 解码 `frame_codec::pack_runs` 的字节流。
/// wire 格式（全部小端）：
/// row_count:u16 → 每行 { run_count:u16 · stable_row:i64 · wrapped:u8 →
///   每个 run { start:u16 · len:u16 · fg · bg · attrs:u8 · text_len:u32 · text } }
/// fg/bg：0=Default；1+u8=Ansi；2+r,g,b=Rgb。
TerminalFrame decodeFrame(
  Uint8List binary, {
  required int cols,
  required int rows,
  required int cursorCol,
  required int cursorRow,
}) {
  final data = ByteData.sublistView(binary);
  var o = 0;
  void need(int n) {
    if (o + n > binary.length) {
      throw FormatException('truncated frame at byte $o (+$n)');
    }
  }

  int u8() {
    need(1);
    return data.getUint8(o++);
  }

  int u16() {
    need(2);
    final v = data.getUint16(o, Endian.little);
    o += 2;
    return v;
  }

  int u32() {
    need(4);
    final v = data.getUint32(o, Endian.little);
    o += 4;
    return v;
  }

  int i64() {
    need(8);
    final v = data.getInt64(o, Endian.little);
    o += 8;
    return v;
  }

  TermColor color() {
    final marker = u8();
    switch (marker) {
      case 0:
        return const DefaultColor();
      case 1:
        return AnsiColor(u8());
      case 2:
        final r = u8();
        final g = u8();
        final b = u8();
        return RgbColor(r, g, b);
      default:
        throw FormatException('bad color marker $marker at byte ${o - 1}');
    }
  }

  final rowCount = u16();
  final lines = <FrameRow>[];
  for (var r = 0; r < rowCount; r++) {
    final runCount = u16();
    final stableRow = i64();
    final wrapped = u8() != 0;
    final runs = <FrameRun>[];
    for (var i = 0; i < runCount; i++) {
      final start = u16();
      final len = u16();
      final fg = color();
      final bg = color();
      final attrs = u8();
      final textLen = u32();
      need(textLen);
      final text = utf8.decode(binary.sublist(o, o + textLen), allowMalformed: true);
      o += textLen;
      runs.add(
        FrameRun(
          start: start,
          len: len,
          attrs: attrs,
          fg: fg,
          bg: bg,
          text: text,
        ),
      );
    }
    lines.add(
      FrameRow(stableRow: stableRow, wrapped: wrapped, runs: runs),
    );
  }
  if (o != binary.length) {
    throw FormatException('trailing bytes: stream=$binary.length consumed=$o');
  }
  return TerminalFrame(
    cols: cols,
    rows: rows,
    cursorCol: cursorCol,
    cursorRow: cursorRow,
    lines: lines,
  );
}
