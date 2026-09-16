import 'package:flutter/material.dart';

import 'frame.dart';
import 'unicode_width.dart';

/// 把 [TerminalFrame] 画到画布上。
///
/// 定位策略：每个 run 按其 `start` 列**绝对定位**——CJK 宽字符
/// 占 2 列也不会让后续 run 漂移；run 内部按字形簇（含零宽贴附）
/// 逐簇定位，保证 CJK/emoji 不错位。
class TerminalPainter extends CustomPainter {
  final TerminalFrame frame;
  final double cellWidth;
  final double cellHeight;
  final double fontSize;

  static const Color defaultBg = Color(0xFF0C0F14);
  static const Color defaultFg = Color(0xFFD6D9DE);

  TerminalPainter({
    required this.frame,
    required this.cellWidth,
    required this.cellHeight,
    required this.fontSize,
  });

  /// ANSI 256 色 → ARGB。0–15 经典色，16–231 6×6×6 立方，232–255 灰阶。
  static Color ansi(int index) {
    const base = [
      Color(0xFF3A3D43), // 0 black
      Color(0xFFCC4444), // 1 red
      Color(0xFF77B769), // 2 green
      Color(0xFFD8A047), // 3 yellow
      Color(0xFF5D88C5), // 4 blue
      Color(0xFFB266B2), // 5 magenta
      Color(0xFF54A8A8), // 6 cyan
      Color(0xFFC8CDD3), // 7 white
      Color(0xFF5A5E66), // 8 bright black
      Color(0xFFE86E6E), // 9 bright red
      Color(0xFF9BD98F), // 10 bright green
      Color(0xFFEDBF70), // 11 bright yellow
      Color(0xFF87A9DC), // 12 bright blue
      Color(0xFFD0A3D0), // 13 bright magenta
      Color(0xFF83CDCD), // 14 bright cyan
      Color(0xFFE6E9ED), // 15 bright white
    ];
    if (index < 16) return base[index];
    if (index < 232) {
      final i = index - 16;
      final r = i ~/ 36, g = (i ~/ 6) % 6, b = i % 6;
      int level(int v) => v == 0 ? 0 : 55 + v * 40;
      return Color.fromARGB(255, level(r), level(g), level(b));
    }
    final gray = 8 + (index - 232) * 10;
    return Color.fromARGB(255, gray, gray, gray);
  }

  Color _resolveFg(TermColor color) => switch (color) {
        DefaultColor() => defaultFg,
        AnsiColor(:final index) => ansi(index),
        RgbColor(:final r, :final g, :final b) => Color.fromARGB(255, r, g, b),
      };

  Color _resolveBg(TermColor color) => switch (color) {
        DefaultColor() => defaultBg,
        AnsiColor(:final index) => ansi(index),
        RgbColor(:final r, :final g, :final b) => Color.fromARGB(255, r, g, b),
      };

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = defaultBg);

    final tp = TextPainter(textDirection: TextDirection.ltr);
    final fillPaint = Paint();
    final linePaint = Paint()..strokeWidth = 1.0;

    for (var rowIndex = 0; rowIndex < frame.lines.length; rowIndex++) {
      final row = frame.lines[rowIndex];
      final dy = rowIndex * cellHeight;
      for (final run in row.runs) {
        var fg = _resolveFg(run.fg);
        var bg = _resolveBg(run.bg);
        if (run.reverse) {
          final swap = fg;
          fg = bg;
          bg = swap;
        }
        final x = run.start * cellWidth;
        final w = run.len * cellWidth;

        if (bg != defaultBg) {
          fillPaint.color = bg;
          canvas.drawRect(Rect.fromLTWH(x, dy, w, cellHeight), fillPaint);
        }

        final blank = run.text.codeUnits.every((c) => c == 0x20);
        if (!blank) {
          if (_isAscii(run.text)) {
            tp.text = TextSpan(
              text: run.text,
              style: _styleFor(run, fg),
            );
            tp.layout();
            tp.paint(canvas, Offset(x, dy));
          } else {
            var cell = run.start;
            for (final cluster in _clusters(run.text)) {
              tp.text = TextSpan(text: cluster.$1, style: _styleFor(run, fg));
              tp.layout();
              tp.paint(canvas, Offset(cell * cellWidth, dy));
              cell += cluster.$2;
            }
          }
        }

        final underlineY = dy + cellHeight - 1.5;
        final strikeY = dy + cellHeight * 0.55;
        if (run.underline) {
          linePaint.color = fg;
          canvas.drawLine(Offset(x, underlineY), Offset(x + w, underlineY), linePaint);
        }
        if (run.strike) {
          linePaint.color = fg;
          canvas.drawLine(Offset(x, strikeY), Offset(x + w, strikeY), linePaint);
        }
      }
    }

    if (frame.cursorCol >= 0 && frame.cursorRow >= 0) {
      final rect = Rect.fromLTWH(
        frame.cursorCol * cellWidth,
        frame.cursorRow * cellHeight,
        cellWidth,
        cellHeight,
      );
      canvas.drawRect(
        rect.deflate(0.75),
        Paint()
          ..color = defaultFg.withValues(alpha: 0.85)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5,
      );
    }
  }

  TextStyle _styleFor(FrameRun run, Color color) => TextStyle(
        fontFamily: 'Menlo',
        fontFamilyFallback: const ['monospace', 'Courier New'],
        fontSize: fontSize,
        color: color,
        fontWeight: run.bold ? FontWeight.w700 : FontWeight.w400,
        fontStyle: run.italic ? FontStyle.italic : FontStyle.normal,
      );

  static bool _isAscii(String text) {
    for (final c in text.codeUnits) {
      if (c < 0x20 || c > 0x7E) return false;
    }
    return true;
  }

  /// 把 run 文本切成字形簇：(字符串, 占列数)。零宽 code point 贴附到前簇。
  static List<(String, int)> _clusters(String text) {
    final out = <(String, int)>[];
    final buf = StringBuffer();
    var bufWidth = 0;
    for (final rune in text.runes) {
      final w = runeCellWidth(rune);
      if (w == 0 && buf.isNotEmpty) {
        buf.writeCharCode(rune);
        continue;
      }
      if (buf.isNotEmpty) {
        out.add((buf.toString(), bufWidth));
        buf.clear();
      }
      buf.writeCharCode(rune);
      bufWidth = w < 1 ? 1 : w;
    }
    if (buf.isNotEmpty) {
      out.add((buf.toString(), bufWidth));
    }
    return out;
  }

  @override
  bool shouldRepaint(TerminalPainter oldDelegate) =>
      !identical(oldDelegate.frame, frame) ||
      oldDelegate.cellWidth != cellWidth ||
      oldDelegate.cellHeight != cellHeight;
}
