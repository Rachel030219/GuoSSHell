import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart'
    show
        TerminalController,
        TerminalStyle,
        TerminalView,
        TerminalThemes,
        SelectionMode,
        CellAnchor;

import 'package:guosh_shell/src/bindings/bindings.dart';

import 'frame.dart';
import 'frame_terminal.dart';
import 'terminal_key_bar.dart';

/// M2 之前选区未实现：拦截 setSelection，让 fork 的长按/拖动手势安全落空。
/// M2a 换回真 controller（届时选区/复制粘贴一起实现）。
class _NoSelectionController extends TerminalController {
  @override
  void setSelection(CellAnchor base, CellAnchor extent, {SelectionMode? mode}) {}
}

const double _fontSize = 14;

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
  final _NoSelectionController _terminalController = _NoSelectionController();

  StreamSubscription? _statusSub;
  StreamSubscription? _frameSub;
  StreamSubscription? _perfSub;

  _Phase _phase = _Phase.form;
  SessionState? _state;
  String _detail = '';
  String _debug = '';

  // 帧序号与丢帧（FrameUpdate.seq 跳变 = 有帧没送达）。
  int? _lastSeq;
  int _dropped = 0;
  PerfStats? _perf;

  // fps 计数（M1 验收的测量工具）。
  int _fps = 0;
  int _frameCount = 0;
  DateTime _fpsStamp = DateTime.now();

  @override
  void initState() {
    super.initState();
    _statusSub = SessionStatus.rustSignalStream.listen(_onStatus);
    _frameSub = FrameUpdate.rustSignalStream.listen(_onFrame);
    _perfSub = PerfStats.rustSignalStream.listen(_onPerf);
    _terminal
      ..onInput = _onTerminalInput
      ..onResize = _onTerminalResize;
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
    _perfSub?.cancel();
    _host.dispose();
    _port.dispose();
    _username.dispose();
    _password.dispose();
    _command.dispose();
    super.dispose();
  }

  void _onStatus(RustSignalPack<SessionStatus> pack) {
    if (!mounted) return;
    setState(() {
      _state = pack.message.state;
      _detail = pack.message.detail;
    });
  }

  void _onFrame(RustSignalPack<FrameUpdate> pack) {
    if (!mounted) return;
    final msg = pack.message;
    var dropped = _dropped;
    final lastSeq = _lastSeq;
    if (lastSeq != null && msg.seq > lastSeq + 1) {
      dropped += msg.seq - lastSeq - 1;
    }
    try {
      final frame = decodeFrame(
        pack.binary,
        cols: msg.cols,
        rows: msg.rows,
        cursorCol: msg.cursorCol,
        cursorRow: msg.cursorRow,
      );
      _terminal.applyFrame(frame);
      setState(() {
        _dropped = dropped;
        _lastSeq = msg.seq;
        _frameCount++;
      });
    } catch (error) {
      // 临时诊断：M2 联调期把帧链路错误显示出来。
      setState(() {
        _dropped = dropped;
        _lastSeq = msg.seq;
        _debug = 'ERR $error';
      });
    }
    final now = DateTime.now();
    if (now.difference(_fpsStamp).inMilliseconds >= 1000) {
      setState(() {
        _fps = _frameCount;
        _frameCount = 0;
        _fpsStamp = now;
      });
    }
  }

  void _onPerf(RustSignalPack<PerfStats> pack) {
    if (!mounted) return;
    setState(() => _perf = pack.message);
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
    }
    return true;
  }

  /// fork 的 render 在布局期报几何 → Rust（度量的权威在 Flutter）。
  /// 首次几何到达时若还有待发的连接请求，就带着真实几何发 ConnectRequest——
  /// 否则远端 PTY 只能按缺省 2×2 建立，exec 输出会在换行历史里塞满垃圾。
  void _onTerminalResize(int cols, int rows, int pixelWidth, int pixelHeight) {
    final changed = !_sentSameGeometry(cols, rows, pixelWidth, pixelHeight);
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
    if (!changed || _state != SessionState.connected) return;
    ResizeRequest(
      cols: cols,
      rows: rows,
      pixelWidth: pixelWidth,
      pixelHeight: pixelHeight,
      dpi: (96 * MediaQuery.devicePixelRatioOf(context)).round(),
    ).sendSignalToRust();
  }

  int _sentCols = 0, _sentRows = 0, _sentPw = 0, _sentPh = 0;

  bool _sentSameGeometry(int cols, int rows, int pw, int ph) {
    if (cols == _sentCols && rows == _sentRows && pw == _sentPw && ph == _sentPh) {
      return true;
    }
    _sentCols = cols;
    _sentRows = rows;
    _sentPw = pw;
    _sentPh = ph;
    return false;
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
      _lastSeq = null;
      _dropped = 0;
      _perf = null;
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
      _lastSeq = null;
      _dropped = 0;
      _pendingConnect = null;
      _perf = null;
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
                      terminal: _terminal,
                      controller: _terminalController,
                    ),
                  ),
                  if (banner != null)
                    Positioned(top: 0, left: 0, right: 0, child: banner),
                  Positioned(
                    right: 10,
                    bottom: 8,
                    child: _PerfOverlay(fps: _fps, dropped: _dropped, perf: _perf, debugText: _debug),
                  ),
                  Positioned(
                    top: 4,
                    right: 4,
                    child: IconButton(
                      onPressed: _backToForm,
                      icon: const Icon(Icons.close, size: 20),
                      tooltip: '断开并返回',
                    ),
                  ),
                ],
              ),
            ),
            TerminalKeyBar(terminal: _terminal),
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

  const _TerminalSurface({
    required this.terminal,
    required this.controller,
  });

  @override
  Widget build(BuildContext context) {
    return TerminalView(
      terminal,
      controller: controller,
      autoResize: true,
      // iOS 软键盘的退格不产生硬件按键事件，必须靠编辑增量探测
      // （fork 的 onDelete → keyInput(backspace)）。
      deleteDetection: true,
      textStyle: TerminalStyle(
        fontSize: _fontSize,
        fontFamily: 'Menlo',
        fontFamilyFallback: ['Menlo', 'monospace', 'Courier New'],
      ),
      theme: TerminalThemes.defaultTheme,
      keyboardType: TextInputType.emailAddress,
      keyboardAppearance: Brightness.dark,
    );
  }
}

class _PerfOverlay extends StatelessWidget {
  final int fps;
  final int dropped;
  final PerfStats? perf;
  final String? debugText;

  const _PerfOverlay({
    required this.fps,
    required this.dropped,
    this.perf,
    this.debugText,
  });

  @override
  Widget build(BuildContext context) {
    const style = TextStyle(
      color: Color(0x59FFFFFF),
      fontSize: 11,
      fontFamily: 'Menlo',
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text('$fps fps', style: style),
        if (dropped > 0) Text('丢帧 $dropped', style: style),
        if (debugText != null) Text(debugText!, style: style),
        if (perf != null) ...[
          Text('pack ${perf!.packUsAvg}/${perf!.packUsMax}μs', style: style),
          Text('render ${perf!.renderUsAvg}/${perf!.renderUsMax}μs', style: style),
          Text('帧均 ${perf!.bytesAvg ~/ 1024}KB', style: style),
        ],
      ],
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
