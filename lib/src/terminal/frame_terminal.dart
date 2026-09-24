import 'package:flutter/foundation.dart';
// 我们的 fork 没把这个类放进公开导出面，但锚点机制依赖它（行身份），
// 属于有意的实现级依赖，fork 收紧导出面前先直接引。
// ignore: implementation_imports
import 'package:terminal_view/src/utils/circular_buffer.dart';
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

/// 触摸鼠标事件（M2a）。仅在远端开启鼠标上报时产生。
class MouseInputEvent extends TerminalInputEvent {
  final TerminalMouseButton button;
  final TerminalMouseButtonState buttonState;
  final CellOffset position;
  const MouseInputEvent(this.button, this.buttonState, this.position);
}

/// 一条池化行：BufferLine 对象 + 它当初被填充时的 run 快照。
/// 快照逐字段比对相等 → 复用对象、不动 version → 行 Picture 缓存命中。
class _PooledLine {
  final BufferLine line;
  final List<FrameRun> runs;
  final bool wrapped;
  _PooledLine(this.line, this.runs, this.wrapped);
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
    _buffer.push(BufferLine(_cols));
  }

  /// 键盘/IME/粘贴的出口。返回值表示事件是否被接受。
  bool Function(TerminalInputEvent event)? onInput;

  /// 视口尺寸变化（列/行/pixel），由 fork 的 render 在布局期回调。
  void Function(int cols, int rows, int pixelWidth, int pixelHeight)? onResize;

  /// 行的身份容器。fork 的 CellAnchor 挂在 BufferLine 对象上，锚点的
  /// 行号 = owner.index，而 index 只有经 IndexAwareCircularBuffer 收养
  /// （attached）才有效——M2a 起所有行都住在这里。
  /// 容量固定富余（1024 行，覆盖 iPad 分屏与 M4 的滚回窗口）；
  /// _length 只能靠 push 长大，帧变高时补齐、变矮时留尾巴（画不到）。
  final IndexAwareCircularBuffer<BufferLine> _buffer =
      IndexAwareCircularBuffer<BufferLine>(1024);

  /// 上一帧的行对象表：key 是帧里的 `stable_row`（引擎的绝对行号）。
  /// 用行身份而不是屏幕位置，内容滚动时对象跟着内容走。
  final Map<int, _PooledLine> _pool = {};

  /// 视口行 → 引擎绝对行（`stable_row`）。选区的坐标换算要用：
  /// fork 给的是视口坐标，引擎要的是绝对行号。M2a 无滚回，两者一一对应。
  final List<int> _stableRows = [];

  /// 绝对行 → 视口行（唯一；帧内不会重复）。收引擎回显时用。
  final Map<int, int> _viewportByStableRow = {};

  bool _mouseReporting = false;
  bool _alternateScreen = false;

  /// 远端鼠标上报（随帧下发）。开启后触摸点击/滚轮转发远端。
  set mouseReporting(bool value) {
    if (_mouseReporting == value) return;
    _mouseReporting = value;
    notifyListeners();
  }

  /// 远端备用屏（随帧下发）。fork 的滚动归属判定要用。
  set alternateScreen(bool value) {
    if (_alternateScreen == value) return;
    _alternateScreen = value;
    notifyListeners();
  }

  int _cols = 80;
  int _rows = 24;
  /// 缓冲里「有效」的行数（= 最新一帧的 rows）。
  /// 不能直接用 `_buffer.length`：帧变矮时旧行还挂在尾部（fork 没暴露
  /// 删行 API），若把物理长度当高度，那些旧行会被当成缓冲的一部分——
  /// 实机症状：键盘弹出后旧的下部内容留在屏上、滚动后重复出现。
  int _height = 1;
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

    // 行池：key = stable_row。同一行的内容没变就复用同一对象（Picture
    // 缓存与选区锚点都靠对象身份）；内容变了才换新对象。
    final next = <int, _PooledLine>{};
    final lines = <BufferLine>[];
    for (final row in frame.lines) {
      final pooled = _reusable(row) ??
          _PooledLine(_buildLine(row), row.runs, row.wrapped);
      next[row.stableRow] = pooled;
      lines.add(pooled.line);
    }
    _pool
      ..clear()
      ..addAll(next);

    // 本帧用到的对象集合：判断某个旧对象的内容是否还在（在 = 锚点已跟着
    // 对象走到新位置，不需要处理）。
    // 行池按位置铺开：新行补进缓冲、位置上的旧对象换成新对象。选区不再挂
    // 锚点（权威在引擎），所以不做锚点迁移。
    _stableRows
      ..clear()
      ..addAll([for (final row in frame.lines) row.stableRow]);
    _viewportByStableRow.clear();
    for (var i = 0; i < _stableRows.length; i++) {
      _viewportByStableRow[_stableRows[i]] = i;
    }

    while (_buffer.length < lines.length) {
      _buffer.push(lines[_buffer.length]);
    }
    for (var i = 0; i < lines.length; i++) {
      _buffer[i] = lines[i];
    }
    _height = lines.length;
    notifyListeners();
  }

  /// 视口行 → 引擎绝对行。越界返回 null。
  int? stableRowAt(int viewportRow) {
    if (viewportRow < 0 || viewportRow >= _stableRows.length) return null;
    return _stableRows[viewportRow];
  }

  /// 引擎绝对行 → 视口行。不在当前视口内返回 null（选区画不出耳朵）。
  int? viewportRowForStable(int stableRow) => _viewportByStableRow[stableRow];

  /// 该 stable_row 上一帧的行对象在内容仍相同时可以复用。
  _PooledLine? _reusable(FrameRow row) {
    final pooled = _pool[row.stableRow];
    if (pooled == null) return null;
    if (pooled.wrapped != row.wrapped) return null;
    if (!_runsEqual(pooled.runs, row.runs)) return null;
    return pooled;
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
    final line = BufferLine(_cols, isWrapped: row.wrapped);
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
  int get height => _height;

  @override
  BufferLine lineAt(int index) => _buffer[index];

  @override
  int get cursorX => _cursorX;

  /// 无滚动缓冲：视口行号即绝对行号（scrollback 留 M4）。
  @override
  int get absoluteCursorY => _cursorY;

  /// 切词（长按选词的边界规则）：与 fork 的 Buffer 同一套分隔符
  /// （NUL/空白/`.`/`:`/`-`/`\`/`"`/`*`/`+`/`/`）。CJK 连续段成一个词。
  static const Set<int> _wordSeparators = {
    0,
    0x20,
    0x2e,
    0x3a,
    0x2d,
    0x5c,
    0x22,
    0x2a,
    0x2b,
    0x2f,
  };

  @override
  BufferRangeLine? getWordBoundary(CellOffset position) {
    if (position.y < 0 || position.y >= _height) return null;
    final line = _buffer[position.y];
    var start = position.x;
    var end = position.x;
    while (start > 0 && !_wordSeparators.contains(line.getCodePoint(start - 1))) {
      start--;
    }
    while (end < _cols && !_wordSeparators.contains(line.getCodePoint(end))) {
      end++;
    }
    if (start == end) return null;
    return BufferRangeLine(CellOffset(start, position.y), CellOffset(end, position.y));
  }

  @override
  CellAnchor createAnchor(int x, int y) {
    final line = _buffer[y.clamp(0, _height - 1)];
    return line.createAnchor(x.clamp(0, _cols - 1));
  }

  @override
  CellAnchor createAnchorFromOffset(CellOffset offset) =>
      createAnchor(offset.x, offset.y);

  @override
  String getText([BufferRange? range]) {
    range ??= BufferRangeLine(
      CellOffset(0, 0),
      CellOffset(_cols - 1, _height - 1),
    );
    range = range.normalized;
    final builder = StringBuffer();
    for (final segment in range.toSegments()) {
      if (segment.line < 0 || segment.line >= _height) continue;
      final line = _buffer[segment.line];
      // 换行规则与 fork 的 Buffer.getText 一致：起始行/首行/wrapped
      // 行前不插换行（软换行的物理行拼回一段）。
      if (!(segment.line == range.begin.y || segment.line == 0 || line.isWrapped)) {
        builder.write('\n');
      }
      // 行尾空格要裁掉：上游在 wire 上把空白格发成**字面空格**
      // （rshell-session render.rs 的 blank_cell 用 " "），fork 的
      // 裁尾逻辑只认内容 0，不裁的话每行都拖着一串到行宽的空格。
      builder.write(line.getText(segment.start, segment.end).trimRight());
    }
    return builder.toString();
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
  MouseMode get mouseMode =>
      _mouseReporting ? MouseMode.clickOnly : MouseMode.none;

  @override
  bool get isUsingAltBuffer => _alternateScreen;

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
    // 鼠标上报没开：不消费，fork 回退到本地行为（聚焦/滚动）。
    if (!_mouseReporting) return false;
    _emit(MouseInputEvent(
      button,
      buttonState,
      CellOffset(position.x.clamp(0, _cols - 1), position.y.clamp(0, _rows - 1)),
    ));
    return true;
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
