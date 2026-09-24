import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart'
    show
        CellOffset,
        TerminalController,
        TerminalStyle,
        TerminalView,
        TerminalThemes,
        TerminalMouseButton,
        TerminalMouseButtonState;
// 字符度量是 fork 的内部工具，但选区菜单的锚点定位要用它
// （与行身份缓冲同一类实现级依赖）。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/char_metrics.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';

import 'frame.dart';
import 'frame_terminal.dart';
import 'terminal_key_bar.dart';

const double _fontSize = 14;

/// 终端字体样式：视图渲染与选区菜单锚点定位共用同一份。
const TerminalStyle _terminalStyle = TerminalStyle(
  fontSize: _fontSize,
  fontFamily: 'Menlo',
  fontFamilyFallback: ['Menlo', 'monospace', 'Courier New'],
);

enum _Phase { form, session }

/// 待发的连接请求：等首次布局几何到达后才真正发出（见 _onTerminalResize）。
class _PendingConnect {
  final String host;
  final int port;
  final String username;
  final String password;
  final String command;

  const _PendingConnect({
    required this.host,
    required this.port,
    required this.username,
    required this.password,
    required this.command,
  });
}

/// 主界面：连接表单 → 终端（fork 渲染 + 帧驱动适配器 + 键位条）。
class TerminalPage extends StatefulWidget {
  final String? autoHost;
  final int? autoPort;
  final String? autoUsername;
  final String? autoPassword;
  final String? autoCommand;

  const TerminalPage({
    super.key,
    this.autoHost,
    this.autoPort,
    this.autoUsername,
    this.autoPassword,
    this.autoCommand,
  });

  @override
  State<TerminalPage> createState() => _TerminalPageState();
}

class _TerminalPageState extends State<TerminalPage> {
  final _host = TextEditingController();
  final _port = TextEditingController(text: '22');
  final _username = TextEditingController();
  final _password = TextEditingController();
  final _command = TextEditingController();

  final FrameTerminal _terminal = FrameTerminal();
  final TerminalController _terminalController = TerminalController();
  /// 软键盘的开关靠它：焦点在终端上 = 键盘起，unfocus = 收起。
  final FocusNode _terminalFocus = FocusNode();
  /// 选区菜单锚点定位用（终端渲染区的屏幕坐标）。
  final GlobalKey _terminalSurfaceKey = GlobalKey();
  /// Flutter 自带的选区菜单（iOS 上是系统风格气垫）。
  final ContextMenuController _selectionMenu = ContextMenuController();

  StreamSubscription? _statusSub;
  StreamSubscription? _frameSub;
  StreamSubscription? _selectionSub;
  StreamSubscription? _clipboardSub;

  /// 引擎最近一次选区回显（绝对行坐标）。视口一变就要拿它重新投影。
  SelectionState? _selectionEcho;
  /// 上次投影时的视口映射（首/末绝对行）。变了才重新投影——
  /// 拖动中的乐观更新不会被迟到的回显顶掉。
  int? _projectedFirstStable;
  int? _projectedLastStable;

  _Phase _phase = _Phase.form;
  SessionState? _state;
  String _detail = '';

  @override
  void initState() {
    super.initState();
    _statusSub = SessionStatus.rustSignalStream.listen(_onStatus);
    _frameSub = FrameUpdate.rustSignalStream.listen(_onFrame);
    _selectionSub = SelectionState.rustSignalStream.listen(_onSelectionState);
    _clipboardSub = ClipboardText.rustSignalStream.listen(_onClipboardText);
    _terminal
      ..onInput = _onTerminalInput
      ..onResize = _onTerminalResize;
    _terminalController
      ..addListener(_onSelectionChanged)
      ..onSelectionIntent = _onSelectionIntent;
    if (widget.autoHost != null && widget.autoHost!.isNotEmpty) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        _sendConnect(
          host: widget.autoHost!,
          port: widget.autoPort ?? 22,
          username: widget.autoUsername ?? '',
          password: widget.autoPassword ?? '',
          command: widget.autoCommand ?? '',
        );
      });
    }
  }

  @override
  void dispose() {
    _statusSub?.cancel();
    _frameSub?.cancel();
    _selectionSub?.cancel();
    _clipboardSub?.cancel();
    _terminalController
      ..removeListener(_onSelectionChanged)
      ..onSelectionIntent = null;
    _selectionMenu.remove();
    _resizeTimer?.cancel();
    _terminalController.dispose();
    _terminalFocus.dispose();
    _host.dispose();
    _port.dispose();
    _username.dispose();
    _password.dispose();
    _command.dispose();
    super.dispose();
  }

  void _onStatus(RustSignalPack<SessionStatus> pack) {
    if (!mounted) return;
    final state = pack.message.state;
    // 会话结束（失败/断开）：引擎那边选区没了，别再留着高亮/耳朵。
    if (state == SessionState.closed || state == SessionState.failed) {
      _selectionEcho = null;
      _projectedFirstStable = null;
      _projectedLastStable = null;
      _terminalController.setExternalSelection(null, null);
    }
    setState(() {
      _state = state;
      _detail = pack.message.detail;
    });
  }

  void _onFrame(RustSignalPack<FrameUpdate> pack) {
    if (!mounted) return;
    final msg = pack.message;
    try {
      final frame = decodeFrame(
        pack.binary,
        cols: msg.cols,
        rows: msg.rows,
        cursorCol: msg.cursorCol,
        cursorRow: msg.cursorRow,
      );
      // 显示模式随帧走（RenderFrame 已带，M2a 起过边界）：
      // 决定触摸点击/滚轮是转发远端还是保持本地行为。
      _terminal
        ..mouseReporting = msg.mouseReporting
        ..alternateScreen = msg.alternateScreen;
      _terminal.applyFrame(frame);
      // 滚动/重排会改 stable→视口 的映射；映射变了就重新投影选区，
      // 高亮和耳朵才会跟着内容走（拖动进行中映射不变，不会被顶掉）。
      final first = _terminal.stableRowAt(0);
      final last =
          _terminal.height > 0 ? _terminal.stableRowAt(_terminal.height - 1) : null;
      if (first != _projectedFirstStable || last != _projectedLastStable) {
        _projectedFirstStable = first;
        _projectedLastStable = last;
        _projectSelection();
      }
      // 重排可能让选区端点失效：这时把还挂着的菜单收回，别留个孤儿气泡。
      if (_selectionMenu.isShown && _terminalController.selection == null) {
        _selectionMenu.remove();
      }
    } catch (error) {
      debugPrint('[frame] decode failed: $error');
    }
  }

  /// 引擎回显选区 → 记住（绝对行坐标）→ 投影到当前视口。
  void _onSelectionState(RustSignalPack<SelectionState> pack) {
    if (!mounted) return;
    _selectionEcho = pack.message;
    _projectSelection();
  }

  /// 把引擎回显的选区（绝对行）投影到当前视口并喂给 fork。
  /// 端点滚出视口时贴边截断（可见部分保留高亮），整体不可见才清空。
  void _projectSelection() {
    final echo = _selectionEcho;
    if (echo == null || !echo.hasSelection) {
      _terminalController.setExternalSelection(null, null);
      return;
    }
    final rows = _terminal.height;
    final firstStable = rows > 0 ? _terminal.stableRowAt(0) : null;
    final lastStable = rows > 0 ? _terminal.stableRowAt(rows - 1) : null;
    if (firstStable == null || lastStable == null) {
      _terminalController.setExternalSelection(null, null);
      return;
    }

    // 排序出首端/尾端（首端 = 早的那个），角色保留给 begin/end。
    final anchorIsFirst = echo.anchorRow < echo.focusRow ||
        (echo.anchorRow == echo.focusRow && echo.anchorCol <= echo.focusCol);
    final firstStableRow = anchorIsFirst ? echo.anchorRow : echo.focusRow;
    final lastStableRow = anchorIsFirst ? echo.focusRow : echo.anchorRow;
    final firstCol = anchorIsFirst ? echo.anchorCol : echo.focusCol;
    final lastCol = anchorIsFirst ? echo.focusCol : echo.anchorCol;

    // 首端滚到视口下方 / 尾端滚到视口上方 = 整个选区都看不见。
    if (firstStableRow > lastStable || lastStableRow < firstStable) {
      _terminalController.setExternalSelection(null, null);
      return;
    }

    final firstRow = _terminal.viewportRowForStable(firstStableRow);
    final firstOffset = firstRow != null
        ? CellOffset(firstCol, firstRow)
        : const CellOffset(0, 0); // 上方滚出：贴到首行首格
    final lastRow = _terminal.viewportRowForStable(lastStableRow);
    final lastOffset = lastRow != null
        ? CellOffset(lastCol, lastRow)
        : CellOffset(0, rows); // 下方滚出：贴到末行之后（排他端）

    _terminalController.setExternalSelection(
      anchorIsFirst ? firstOffset : lastOffset,
      anchorIsFirst ? lastOffset : firstOffset,
    );
  }

  /// 引擎取文回来 → 进剪贴板 + 清选区（对齐 Termux）。
  void _onClipboardText(RustSignalPack<ClipboardText> pack) {
    if (!mounted) return;
    Clipboard.setData(ClipboardData(text: pack.message.text));
    _sendSelectionRequest(clear: true);
  }

  /// fork 上报的选区意图 → 换算成引擎绝对行 → 发 SelectionRequest。
  void _onSelectionIntent(CellOffset? begin, CellOffset? end) {
    if (_state != SessionState.connected) return;
    if (begin == null || end == null) {
      _sendSelectionRequest(clear: true);
      return;
    }
    final anchorRow = _terminal.stableRowAt(begin.y);
    final focusRow = _terminal.stableRowAt(end.y);
    if (anchorRow == null || focusRow == null) return;
    SelectionRequest(
      clear: false,
      anchorRow: anchorRow,
      anchorCol: begin.x,
      focusRow: focusRow,
      focusCol: end.x,
      rectangular: false,
    ).sendSignalToRust();
  }

  void _sendSelectionRequest({required bool clear}) {
    SelectionRequest(
      clear: clear,
      anchorRow: 0,
      anchorCol: 0,
      focusRow: 0,
      focusCol: 0,
      rectangular: false,
    ).sendSignalToRust();
  }

  /// fork 的输入口 → rinf → Rust（键编码权威在 encode_input）。
  bool _onTerminalInput(TerminalInputEvent event) {
    if (_state != SessionState.connected) return false;
    switch (event) {
      case KeyInputEvent(:final key, :final shift, :final control, :final alt):
        InputRequest(text: '', key: key, shift: shift, control: control, alt: alt)
            .sendSignalToRust();
      case TextInputEvent(:final text):
        InputRequest(text: text, key: '', shift: false, control: false, alt: false)
            .sendSignalToRust();
      case MouseInputEvent(:final button, :final buttonState, :final position):
        // 滚轮走 Scroll（上游 validate 拒绝「滚轮走 press」）；
        // 下压/抬起是 Press/Release。
        final isWheel = button.isWheel;
        final kind = isWheel
            ? 'scroll'
            : switch (buttonState) {
                TerminalMouseButtonState.down => 'press',
                TerminalMouseButtonState.up => 'release',
              };
        MouseRequest(
          kind: kind,
          button: switch (button) {
            TerminalMouseButton.left => 'left',
            TerminalMouseButton.middle => 'middle',
            TerminalMouseButton.right => 'right',
            TerminalMouseButton.wheelUp => 'wheel_up',
            TerminalMouseButton.wheelDown => 'wheel_down',
            _ => 'left',
          },
          col: position.x,
          row: position.y,
          shift: false,
          control: false,
          alt: false,
        ).sendSignalToRust();
    }
    return true;
  }

  /// fork 的 render 在布局期报几何 → Rust（度量的权威在 Flutter）。
  /// 首次几何到达时若还有待发的连接请求，就带着真实几何发 ConnectRequest——
  /// 否则远端 PTY 只能按缺省 2×2 建立，exec 输出会在换行历史里塞满垃圾。
  ///
  /// 软键盘/旋转动画期间 fork 会**逐帧**报新几何；每一步都转发的话，
  /// 远端 shell 会为每一步重画一次提示符（实机实测：一次键盘弹出刷了
  /// 8 行提示符）。去抖后只发最终尺寸。
  void _onTerminalResize(int cols, int rows, int pixelWidth, int pixelHeight) {
    final pending = _pendingConnect;
    if (pending != null) {
      _pendingConnect = null;
      ConnectRequest(
        host: pending.host,
        port: pending.port,
        username: pending.username,
        password: pending.password,
        command: pending.command,
        cols: cols,
        rows: rows,
        pixelWidth: pixelWidth,
        pixelHeight: pixelHeight,
        dpi: (96 * MediaQuery.devicePixelRatioOf(context)).round(),
      ).sendSignalToRust();
      return;
    }
    if (_state != SessionState.connected) return;
    if (cols == _sentCols && rows == _sentRows && pixelWidth == _sentPw && pixelHeight == _sentPh) {
      return;
    }
    _pendingCols = cols;
    _pendingRows = rows;
    _pendingPw = pixelWidth;
    _pendingPh = pixelHeight;
    _resizeTimer?.cancel();
    _resizeTimer = Timer(const Duration(milliseconds: 150), _flushResize);
  }

  void _flushResize() {
    _resizeTimer = null;
    if (!mounted || _state != SessionState.connected) return;
    _sentCols = _pendingCols;
    _sentRows = _pendingRows;
    _sentPw = _pendingPw;
    _sentPh = _pendingPh;
    ResizeRequest(
      cols: _sentCols,
      rows: _sentRows,
      pixelWidth: _sentPw,
      pixelHeight: _sentPh,
      dpi: (96 * MediaQuery.devicePixelRatioOf(context)).round(),
    ).sendSignalToRust();
  }

  int _sentCols = 0, _sentRows = 0, _sentPw = 0, _sentPh = 0;
  int _pendingCols = 0, _pendingRows = 0, _pendingPw = 0, _pendingPh = 0;
  Timer? _resizeTimer;

  /// 软键盘开关（键位条「⌨」键）：焦点在终端 = 键盘起，否则收起。
  void _toggleKeyboard() {
    if (_terminalFocus.hasFocus) {
      _terminalFocus.unfocus();
    } else {
      _terminalFocus.requestFocus();
    }
  }

  /// 选区变化 → 系统风格的选区菜单（Flutter 自带，iOS 上渲染成气垫）。
  /// 选区清空时收起。
  void _onSelectionChanged() {
    if (!mounted) return;
    if (_terminalController.selection == null) {
      _selectionMenu.remove();
      return;
    }
    _selectionMenu.show(
      context: context,
      contextMenuBuilder: (context) => AdaptiveTextSelectionToolbar.buttonItems(
        anchors: _selectionAnchors(),
        buttonItems: [
          // 用官方的按钮类型，文案/样式由 Flutter 按平台给（不再硬编码中文）。
          ContextMenuButtonItem(
            type: ContextMenuButtonType.copy,
            onPressed: () {
              _selectionMenu.remove();
              _copySelection();
            },
          ),
          ContextMenuButtonItem(
            type: ContextMenuButtonType.paste,
            onPressed: () {
              _selectionMenu.remove();
              _pasteClipboard();
            },
          ),
        ],
      ),
    );
  }

  /// 选区端点 → 屏幕锚点。单元格尺寸用 fork 同一套字符度量算，
  /// 终端区原点取渲染盒的全局坐标（无滚动偏移，M2a 视口即缓冲）。
  TextSelectionToolbarAnchors _selectionAnchors() {
    final selection = _terminalController.selection!.normalized;
    final box =
        _terminalSurfaceKey.currentContext?.findRenderObject() as RenderBox?;
    if (box == null) {
      return const TextSelectionToolbarAnchors(primaryAnchor: Offset.zero);
    }
    final origin = box.localToGlobal(Offset.zero);
    final cell = calcCharSize(_terminalStyle, MediaQuery.textScalerOf(context));
    final begin = selection.begin;
    final end = selection.end;
    return TextSelectionToolbarAnchors(
      primaryAnchor:
          origin + Offset(begin.x * cell.width, begin.y * cell.height),
      secondaryAnchor: origin +
          Offset((end.x + 1) * cell.width, (end.y + 1) * cell.height),
    );
  }

  /// 复制选区：取文在引擎里（Dart 不碰 BufferLine），发请求等 ClipboardText。
  void _copySelection() {
    if (_state != SessionState.connected) return;
    CopyRequest().sendSignalToRust();
  }

  /// 系统剪贴板 → 远端（直发原文；bracketed paste 见 followup）。
  Future<void> _pasteClipboard() async {
    final text = (await Clipboard.getData('text/plain'))?.text;
    if (text == null || text.isEmpty) return;
    _terminal.paste(text);
  }

  void _connect() {
    FocusManager.instance.primaryFocus?.unfocus();
    _sendConnect(
      host: _host.text.trim(),
      port: int.tryParse(_port.text.trim()) ?? 22,
      username: _username.text,
      password: _password.text,
      command: _command.text.trim(),
    );
  }

  void _sendConnect({
    required String host,
    required int port,
    required String username,
    required String password,
    required String command,
  }) {
    setState(() {
      _phase = _Phase.session;
      _state = SessionState.connecting;
      _detail = '$host:$port';
      // 不立刻发请求：等会话视图完成首次布局、拿到真实几何再发
      // （见 _onTerminalResize）。
      _pendingConnect = _PendingConnect(
        host: host,
        port: port,
        username: username,
        password: password,
        command: command,
      );
    });
  }

  _PendingConnect? _pendingConnect;

  void _backToForm() {
    DisconnectRequest().sendSignalToRust();
    setState(() {
      _phase = _Phase.form;
      _state = null;
      _pendingConnect = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return _phase == _Phase.form ? _buildForm(context) : _buildSession(context);
  }

  Widget _buildForm(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('GuoSSHell')),
      body: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          TextField(
            controller: _host,
            keyboardType: TextInputType.url,
            autofillHints: const [AutofillHints.url],
            decoration: const InputDecoration(
              labelText: '主机',
              hintText: '192.168.x.x 或域名',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _port,
            keyboardType: TextInputType.number,
            decoration: const InputDecoration(
              labelText: '端口',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _username,
            autofillHints: const [AutofillHints.username],
            decoration: const InputDecoration(
              labelText: '用户名',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _password,
            obscureText: true,
            autofillHints: const [AutofillHints.password],
            decoration: const InputDecoration(
              labelText: '密码',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _command,
            decoration: const InputDecoration(
              labelText: '命令（可选）',
              hintText: '填了就连上直接执行，如 top；留空进 shell',
              border: OutlineInputBorder(),
            ),
          ),
          const SizedBox(height: 20),
          FilledButton.icon(
            onPressed: _connect,
            icon: const Icon(Icons.terminal),
            label: const Text('连接'),
          ),
          if (_detail.isNotEmpty) ...[
            const SizedBox(height: 16),
            Text(
              _detail,
              style: TextStyle(
                color: Theme.of(context).colorScheme.error,
                fontSize: 12,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildSession(BuildContext context) {
    final banner = switch (_state) {
      SessionState.connecting => const _Banner(icon: Icons.sync, text: '连接中…'),
      SessionState.failed => _Banner(
          icon: Icons.error_outline,
          text: '失败：$_detail',
          error: true,
          onBack: _backToForm,
        ),
      SessionState.closed => _Banner(
          icon: Icons.link_off,
          text: '已断开：$_detail',
          onBack: _backToForm,
        ),
      _ => null,
    };

    return Scaffold(
      backgroundColor: Colors.black,
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: Stack(
                children: [
                  Positioned.fill(
                    child: _TerminalSurface(
                      key: _terminalSurfaceKey,
                      terminal: _terminal,
                      controller: _terminalController,
                      focusNode: _terminalFocus,
                    ),
                  ),
                  // 诊断浮层与右上角关闭键已移除：它们悬在终端上方，
                  // 拖选区（尤其长选区）经过时会干扰触摸。
                  if (banner != null)
                    Positioned(top: 0, left: 0, right: 0, child: banner),
                ],
              ),
            ),
            TerminalKeyBar(
              terminal: _terminal,
              extraListen: _terminalController,
              canCopy: () => _terminalController.selection != null,
              onCopy: _copySelection,
              onPaste: _pasteClipboard,
              onToggleKeyboard: _toggleKeyboard,
            ),
          ],
        ),
      ),
    );
  }
}

/// fork 的 TerminalView 需要有界高度（内部是 Scrollable）。
/// 会话期常驻（连接前也要完成首次布局，几何才能随 ConnectRequest 发出）。
class _TerminalSurface extends StatelessWidget {
  final FrameTerminal terminal;
  final TerminalController controller;
  final FocusNode focusNode;

  const _TerminalSurface({
    super.key,
    required this.terminal,
    required this.controller,
    required this.focusNode,
  });

  @override
  Widget build(BuildContext context) {
    return TerminalView(
      terminal,
      controller: controller,
      focusNode: focusNode,
      autoResize: true,
      // iOS 软键盘的退格不产生硬件按键事件，必须靠编辑增量探测
      // （fork 的 onDelete → keyInput(backspace)）。
      deleteDetection: true,
      textStyle: _terminalStyle,
      theme: TerminalThemes.defaultTheme,
      keyboardType: TextInputType.emailAddress,
      keyboardAppearance: Brightness.dark,
    );
  }
}

class _Banner extends StatelessWidget {
  final IconData icon;
  final String text;
  final bool error;
  final VoidCallback? onBack;

  const _Banner({
    required this.icon,
    required this.text,
    this.error = false,
    this.onBack,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: error ? scheme.errorContainer : scheme.surface.withValues(alpha: 0.92),
      child: SafeArea(
        bottom: false,
        child: ListTile(
          dense: true,
          leading: Icon(icon, color: error ? scheme.onErrorContainer : null),
          title: Text(
            text,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(color: error ? scheme.onErrorContainer : null),
          ),
          trailing:
              onBack == null ? null : TextButton(onPressed: onBack, child: const Text('返回')),
        ),
      ),
    );
  }
}
