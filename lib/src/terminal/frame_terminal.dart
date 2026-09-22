import 'package:flutter/foundation.dart';
import 'package:terminal_view/terminal_view.dart';

import 'frame.dart';
import 'unicode_width.dart';

/// 输入事件（适配器 → 页面 → rinf → Rust）。
sealed class TerminalInputEvent {
  const TerminalInputEvent();
}

class KeyInputEvent extends TerminalInputEvent {
  /// "character:x" 或命名键（见 session.rs 的 parse_key_code）。
  final String key;
  final bool shift;
  final bool control;
  final bool alt;
  const KeyInputEvent(this.key, {this.shift = false, this.control = false, this.alt = false});
}

class TextInputEvent extends TerminalInputEvent {
  final String text;
  const TextInputEvent(this.text);
}

/// 一条池化行：BufferLine 对象 + 它当初被填充时的 run 快照。
/// 快照逐字段比对相等 → 复用对象、不动 version → 行 Picture 缓存命中。
class _PooledLine {
  final BufferLine line;
  final List<FrameRun> runs;
  _PooledLine(this.line, this.runs);
}

/// `TerminalSurface` 的帧驱动实现（PLAN §6.1 M2 fork 决定，方案 A）。
///
/// Rust 每帧推来的压缩 run 解码后填进池化的 `BufferLine`：
/// * 相邻同属性单元格的合并已在 Rust 侧完成，这里只负责「展开」回格子；
/// * 行内容逐 run 比对，相同则复用同一对象、不碰 `version`——
///   fork 的行 Picture 重放靠这个命中，60fps 下的主要收益来源；
/// * 终端状态（网格/滚动/模式）全部在 Rust，这里没有任何状态机，
///   只有最新一帧的投影（铁律 4）。
class FrameTerminal with ChangeNotifier implements TerminalSurface, TerminalBufferSurface {
  @override
  TerminalBufferSurface get buffer => this;

  FrameTerminal({this.onInput, this.onResize}) {
    // 首帧前也要可画：fork 的 paint 会对 height-1 做 clamp，height 为 0
    // 直接越界（上游 Terminal 永远至少一行，从没暴露过这个边界）。
    _lines.add(BufferLine(_cols));
  }

  /// 键盘/IME/粘贴的出口。返回值表示事件是否被接受。
  bool Function(TerminalInputEvent event)? onInput;

  /// 视口尺寸变化（列/行/pixel），由 fork 的 render 在布局期回调。
  void Function(int cols, int rows, int pixelWidth, int pixelHeight)? onResize;

  final List<BufferLine> _lines = [];
  final Map<int, _PooledLine> _pool = {};
  int _cols = 80;
  int _rows = 24;
  int _cursorX = 0;
  int _cursorY = 0;
  bool _cursorVisible = false;
  int _sentCols = 0;
  int _sentRows = 0;

  // ── 帧摄入 ──

  void applyFrame(TerminalFrame frame) {
    _cols = frame.cols;
    _rows = frame.rows;
    _cursorX = frame.cursorCol.clamp(0, frame.cols - 1);
    _cursorY = frame.cursorRow.clamp(0, frame.rows - 1);
    _cursorVisible = frame.cursorCol >= 0 && frame.cursorRow >= 0;

    final next = <int, _PooledLine>{};
    final lines = <BufferLine>[];
    for (final row in frame.lines) {
      final signature = _signatureOf(row.runs);
      // 同签名可能对应多行（整屏空白行），共享同一个对象反而让
      // 它们的 Picture 也共享——fork 的缓存按对象身份命中。
      var pooled = next[signature] ?? _pool[signature];
      if (pooled == null || !_runsEqual(pooled.runs, row.runs)) {
        pooled = _PooledLine(_buildLine(row), row.runs);
      }
      next[signature] = pooled;
      lines.add(pooled.line);
    }
    _pool
      ..clear()
      ..addAll(next);
    _lines
      ..clear()
      ..addAll(lines);
    notifyListeners();
  }

  /// run 快照的快速签名（内容比对仍会做全量校验，防哈希碰撞）。
  static int _signatureOf(List<FrameRun> runs) {
    var hash = 0x1fffffff & runs.length;
    for (final run in runs) {
      hash = 0x1fffffff & (hash * 31 + run.start);
      hash = 0x1fffffff & (hash * 31 + run.len);
      hash = 0x1fffffff & (hash * 31 + run.attrs);
      hash = 0x1fffffff & (hash * 31 + run.text.hashCode);
      hash = 0x1fffffff & (hash * 31 + run.fg.hashCode);
      hash = 0x1fffffff & (hash * 31 + run.bg.hashCode);
    }
    return hash;
  }

  static bool _runsEqual(List<FrameRun> a, List<FrameRun> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      final x = a[i];
      final y = b[i];
      if (x.start != y.start ||
          x.len != y.len ||
          x.attrs != y.attrs ||
          x.text != y.text ||
          x.fg.runtimeType != y.fg.runtimeType ||
          x.bg.runtimeType != y.bg.runtimeType) {
        return false;
      }
      if (x.fg is AnsiColor && (x.fg as AnsiColor).index != (y.fg as AnsiColor).index) {
        return false;
      }
      if (x.bg is AnsiColor && (x.bg as AnsiColor).index != (y.bg as AnsiColor).index) {
        return false;
      }
      if (x.fg is RgbColor &&
          ((x.fg as RgbColor).r != (y.fg as RgbColor).r ||
              (x.fg as RgbColor).g != (y.fg as RgbColor).g ||
              (x.fg as RgbColor).b != (y.fg as RgbColor).b)) {
        return false;
      }
      if (x.bg is RgbColor &&
          ((x.bg as RgbColor).r != (y.bg as RgbColor).r ||
              (x.bg as RgbColor).g != (y.bg as RgbColor).g ||
              (x.bg as RgbColor).b != (y.bg as RgbColor).b)) {
        return false;
      }
    }
    return true;
  }

  /// 把一行 run 展开回 BufferLine：按列填充，宽字符占 2 列（第二列
  /// 只带颜色不带字形），run 覆盖范围内剩余列留空白（内容 0、颜色同 run）。
  BufferLine _buildLine(FrameRow row) {
    final line = BufferLine(_cols);
    for (final run in row.runs) {
      final fg = _encodeColor(run.fg, foreground: true);
      final bg = _encodeColor(run.bg, foreground: false);
      final attrs = _encodeAttrs(run);
      final end = (run.start + run.len).clamp(0, _cols);

      var col = run.start;
      final buffer = StringBuffer();
      for (final rune in run.text.runes) {
        final width = runeCellWidth(rune);
        if (width == 0 && buffer.isNotEmpty) {
          buffer.writeCharCode(rune); // 零宽：贴附到前一个字形
          continue;
        }
        if (buffer.isNotEmpty) {
          _fillCluster(line, col, buffer.toString(), 1, fg, bg, attrs);
          col += 1;
          buffer.clear();
        }
        if (col >= end) break;
        final clusterWidth = width < 1 ? 1 : width;
        buffer.writeCharCode(rune);
        _fillCluster(line, col, buffer.toString(), clusterWidth, fg, bg, attrs);
        col += clusterWidth;
        buffer.clear();
        if (col >= end) break;
      }
      if (buffer.isNotEmpty && col < end) {
        _fillCluster(line, col, buffer.toString(), 1, fg, bg, attrs);
        col += 1;
      }
      // run 内剩余列：空白，但保留 run 的底色（反显/色块场景需要）。
      while (col < end) {
        _setColors(line, col, fg, bg, attrs);
        col += 1;
      }
    }
    return line;
  }

  void _fillCluster(
    BufferLine line,
    int col,
    String cluster,
    int width,
    int fg,
    int bg,
    int attrs,
  ) {
    if (col >= _cols) return;
    _setColors(line, col, fg, bg, attrs);
    final codepoints = cluster.runes.toList(growable: false);
    line.setContent(col, codepoints.first | (width << CellContent.widthShift));
    if (codepoints.length > 1) {
      line.setCombined(col, String.fromCharCodes(codepoints.sublist(1)));
    }
    // 宽字符的第二列：只有颜色（painter 跳过 codepoint 0 的字形）。
    if (width == 2 && col + 1 < _cols) {
      _setColors(line, col + 1, fg, bg, attrs);
    }
  }

  void _setColors(BufferLine line, int col, int fg, int bg, int attrs) {
    if (col >= _cols) return;
    line.setForeground(col, fg);
    line.setBackground(col, bg);
    line.setAttributes(col, attrs);
  }

  /// 我们的 Color（Default/Ansi/Rgb）→ fork 的 CellColor 编码。
  static int _encodeColor(TermColor color, {required bool foreground}) {
    switch (color) {
      case DefaultColor():
        return CellColor.normal;
      case AnsiColor(:final index):
        return index < 16
            ? (CellColor.named | index)
            : (CellColor.palette | index);
      case RgbColor(:final r, :final g, :final b):
        return CellColor.rgb | (r << 16) | (g << 8) | b;
    }
  }

  static int _encodeAttrs(FrameRun run) {
    var flags = 0;
    if (run.bold) flags |= CellFlags.bold;
    if (run.italic) flags |= CellFlags.italic;
    if (run.underline) flags |= CellFlags.underline;
    if (run.strike) flags |= CellFlags.strikethrough;
    if (run.reverse) flags |= CellFlags.inverse;
    return flags;
  }

  // ── TerminalBufferSurface（adapter 自身兼任）──

  @override
  int get height => _lines.length;

  @override
  BufferLine lineAt(int index) => _lines[index];

  @override
  int get cursorX => _cursorX;

  /// 无滚动缓冲：视口行号即绝对行号（scrollback 留 M4）。
  @override
  int get absoluteCursorY => _cursorY;

  /// M2a 之前选区未接：null 让 fork 走「单格选区」回退路径，
  /// 页面侧用 no-op 选区 controller 兜住（见 terminal_page.dart）。
  @override
  BufferRangeLine? getWordBoundary(CellOffset position) => null;

  @override
  CellAnchor createAnchor(int x, int y) =>
      CellAnchor(x.clamp(0, _cols - 1), owner: _lineAtOrNull(y));

  @override
  CellAnchor createAnchorFromOffset(CellOffset offset) =>
      CellAnchor(offset.x.clamp(0, _cols - 1), owner: _lineAtOrNull(offset.y));

  BufferLine? _lineAtOrNull(int y) =>
      y >= 0 && y < _lines.length ? _lines[y] : null;

  @override
  String getText([BufferRange? range]) {
    // M2a 实现按选区取文；M2 里选区被 no-op controller 拦住，走不到这里。
    throw UnimplementedError('selection text lands in M2a');
  }

  // ── TerminalSurface ──

  @override
  int get viewWidth => _cols;

  @override
  int get viewHeight => _rows;

  @override
  bool get cursorVisibleMode => _cursorVisible;

  final CursorStyle _cursorStyle = CursorStyle();

  /// 引擎的光标样式随帧走（FrameUpdate 暂未携带），M2 固定块状。
  @override
  CursorStyle get cursor => _cursorStyle;

  @override
  MouseMode get mouseMode => MouseMode.none; // 鼠标转发是 M2a

  @override
  bool get isUsingAltBuffer => false; // alt 信息随帧下发是 M2a 的事

  @override
  void resize(int newWidth, int newHeight, [int? pixelWidth, int? pixelHeight]) {
    // fork 的 render 在布局期调用；只在几何真正变化时转发 Rust，
    // 否则每个布局帧都会触发一次 window-change。
    if (newWidth == _sentCols && newHeight == _sentRows) return;
    _sentCols = newWidth;
    _sentRows = newHeight;
    final callback = onResize;
    if (callback != null) {
      callback(newWidth, newHeight, pixelWidth ?? 0, pixelHeight ?? 0);
    }
  }

  // ── 修饰键挂住/锁定（Termux 式，键位条与软键盘输入共享状态）──
  // 状态必须放在适配器里：软键盘的字母经 TerminalView → onInsert →
  // keyInput 进来，不经过键位条——挂在键位条上的 Ctrl 只有在这里
  // 消费才能作用到任何来源的按键。
  static const String _modCtrl = 'ctrl';
  static const String _modAlt = 'alt';
  final Set<String> _latchedModifiers = {};
  final Set<String> _lockedModifiers = {};

  /// 点击修饰键：未挂 → 挂住一次；挂住 → 锁定；锁定 → 解除。
  void tapModifier(String modifier) {
    if (_lockedModifiers.remove(modifier)) {
      _latchedModifiers.remove(modifier);
    } else if (_latchedModifiers.remove(modifier)) {
      _lockedModifiers.add(modifier);
    } else {
      _latchedModifiers.add(modifier);
    }
    notifyListeners();
  }

  /// 长按修饰键：直接锁定。
  void lockModifier(String modifier) {
    _latchedModifiers.remove(modifier);
    _lockedModifiers.add(modifier);
    notifyListeners();
  }

  bool isModifierLatched(String modifier) => _latchedModifiers.contains(modifier);
  bool isModifierLocked(String modifier) => _lockedModifiers.contains(modifier);

  /// 下一个输入是否带着该修饰键；挂住态在此消耗（锁定态保留）。
  bool _consumeModifier(String modifier) {
    if (_lockedModifiers.contains(modifier)) return true;
    final had = _latchedModifiers.remove(modifier);
    if (had) notifyListeners();
    return had;
  }

  @override
  bool keyInput(
    TerminalKey key, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    final name = _keyName(key);
    if (name == null) return false;
    return _emit(
      KeyInputEvent(
        name,
        shift: shift,
        control: ctrl || _consumeModifier(_modCtrl),
        alt: alt || _consumeModifier(_modAlt),
      ),
    );
  }

  @override
  void textInput(String text) {
    // 挂住的 Ctrl/Alt 作用于下一个输入；单个字符时升级成 Key 事件
    // （软键盘 C + 挂住 Ctrl = Ctrl+C，Rust 侧编码成 ETX）。
    // 多字符文本不消耗挂住态——那是输入法的整段提交，不是「下一个按键」。
    if (text.runes.length == 1) {
      final ctrl = _consumeModifier(_modCtrl);
      final alt = _consumeModifier(_modAlt);
      if (ctrl || alt) {
        _emit(KeyInputEvent('character:$text', control: ctrl, alt: alt));
        return;
      }
    }
    _emit(TextInputEvent(text));
  }

  @override
  void paste(String text) {
    // 粘贴按提交文本转发；bracketed paste 由 M2a 决定。
    _emit(TextInputEvent(text));
  }

  @override
  bool mouseInput(
    TerminalMouseButton button,
    TerminalMouseButtonState buttonState,
    CellOffset position,
  ) {
    return false; // 鼠标转发是 M2a
  }

  bool _emit(TerminalInputEvent event) {
    final callback = onInput;
    if (callback == null) return false;
    return callback(event);
  }

  /// TerminalKey → 边界键名。终端里真正会流经这里的键有限：
  /// 打字走 textInput（IME/软键盘提交），这里只处理功能键与方向键。
  static String? _keyName(TerminalKey key) {
    final named = _namedKeyNames[key];
    if (named != null) return named;
    final fKey = _fKeyNames[key];
    if (fKey != null) return fKey;
    // 字母段在枚举里连续（keyA..keyZ），直接按序号换算。
    if (key.index >= TerminalKey.keyA.index &&
        key.index <= TerminalKey.keyZ.index) {
      return 'character:${String.fromCharCode(0x61 + key.index - TerminalKey.keyA.index)}';
    }
    // 数字段是倒序的（digit9 在 digit0 前面，HID 用法码顺序）。
    if (key.index >= TerminalKey.digit9.index &&
        key.index <= TerminalKey.digit0.index) {
      return 'character:${String.fromCharCode(0x30 + TerminalKey.digit0.index - key.index)}';
    }
    return null;
  }

  static const Map<TerminalKey, String> _namedKeyNames = {
    TerminalKey.enter: 'enter',
    TerminalKey.escape: 'escape',
    TerminalKey.tab: 'tab',
    TerminalKey.backspace: 'backspace',
    TerminalKey.delete: 'delete',
    TerminalKey.insert: 'insert',
    TerminalKey.home: 'home',
    TerminalKey.end: 'end',
    TerminalKey.pageUp: 'page_up',
    TerminalKey.pageDown: 'page_down',
    TerminalKey.arrowUp: 'arrow_up',
    TerminalKey.arrowDown: 'arrow_down',
    TerminalKey.arrowLeft: 'arrow_left',
    TerminalKey.arrowRight: 'arrow_right',
    TerminalKey.space: 'character: ',
  };

  static const Map<TerminalKey, String> _fKeyNames = {
    TerminalKey.f1: 'f:1',
    TerminalKey.f2: 'f:2',
    TerminalKey.f3: 'f:3',
    TerminalKey.f4: 'f:4',
    TerminalKey.f5: 'f:5',
    TerminalKey.f6: 'f:6',
    TerminalKey.f7: 'f:7',
    TerminalKey.f8: 'f:8',
    TerminalKey.f9: 'f:9',
    TerminalKey.f10: 'f:10',
    TerminalKey.f11: 'f:11',
    TerminalKey.f12: 'f:12',
  };
}
