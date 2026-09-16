import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:rinf/rinf.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';

import 'frame.dart';
import 'terminal_painter.dart';

enum _Phase { form, session }

/// M1 主界面：连接表单 → 只读终端画面（帧由 Rust 推送）。
/// Dart 侧唯一的「逻辑」是把帧字节解成 run 并画出来——不解析 ANSI、
/// 不持有终端状态（PLAN.md 铁律 4）。
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
  static const double _fontSize = 14;

  final _host = TextEditingController();
  final _port = TextEditingController(text: '22');
  final _username = TextEditingController();
  final _password = TextEditingController();
  final _command = TextEditingController();

  StreamSubscription? _statusSub;
  StreamSubscription? _frameSub;
  StreamSubscription? _perfSub;

  _Phase _phase = _Phase.form;
  SessionState? _state;
  String _detail = '';
  TerminalFrame? _frame;

  // 帧序号与丢帧（FrameUpdate.seq 跳变 = 有帧没送达）。
  int? _lastSeq;
  int _dropped = 0;
  PerfStats? _perf;

  // 度量与几何（度量的权威在 Flutter，PLAN.md §8）。
  double? _cellWidth;
  double? _cellHeight;
  int _sentCols = 0, _sentRows = 0, _sentPw = 0, _sentPh = 0, _sentDpi = 0;

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
      setState(() {
        _frame = frame;
        _dropped = dropped;
        _lastSeq = msg.seq;
        _frameCount++;
      });
    } on FormatException catch (error) {
      setState(() {
        _dropped = dropped;
        _lastSeq = msg.seq;
        _detail = 'frame decode: $error';
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
      _frame = null;
      _lastSeq = null;
      _dropped = 0;
      _perf = null;
    });
    ConnectRequest(
      host: host,
      port: port,
      username: username,
      password: password,
      command: command,
      cols: _sentCols,
      rows: _sentRows,
      pixelWidth: _sentPw,
      pixelHeight: _sentPh,
      dpi: _sentDpi,
    ).sendSignalToRust();
  }

  void _backToForm() {
    DisconnectRequest().sendSignalToRust();
    setState(() {
      _phase = _Phase.form;
      _frame = null;
      _state = null;
      _lastSeq = null;
      _dropped = 0;
      _perf = null;
    });
  }

  ({double width, double height}) _measureCell() {
    if (_cellWidth == null || _cellHeight == null) {
      final tp = TextPainter(
        text: const TextSpan(
          text: 'W',
          style: TextStyle(fontFamily: 'Menlo', fontSize: _fontSize),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      _cellWidth = tp.width;
      _cellHeight = tp.height;
      tp.dispose();
    }
    return (width: _cellWidth!, height: _cellHeight!);
  }

  /// 画布尺寸变化 → 计算列/行数 → 回传 Rust（engine + 远端 window-change）。
  /// 只在几何真正变化时发送，避免循环。
  void _syncGeometry(BoxConstraints constraints) {
    final cell = _measureCell();
    final cols = (constraints.maxWidth / cell.width).floor().clamp(2, 500);
    final rows = (constraints.maxHeight / cell.height).floor().clamp(2, 300);
    final dpr = MediaQuery.devicePixelRatioOf(context);
    final pw = (cols * cell.width * dpr).round();
    final ph = (rows * cell.height * dpr).round();
    final dpi = (96 * dpr).round();
    if (cols == _sentCols &&
        rows == _sentRows &&
        pw == _sentPw &&
        ph == _sentPh &&
        dpi == _sentDpi) {
      return;
    }
    _sentCols = cols;
    _sentRows = rows;
    _sentPw = pw;
    _sentPh = ph;
    _sentDpi = dpi;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      ResizeRequest(
        cols: cols,
        rows: rows,
        pixelWidth: pw,
        pixelHeight: ph,
        dpi: dpi,
      ).sendSignalToRust();
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
      SessionState.connecting => const _Banner(
          icon: Icons.sync,
          text: '连接中…',
        ),
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
      backgroundColor: TerminalPainter.defaultBg,
      body: SafeArea(
        child: Stack(
          children: [
            Positioned.fill(
              child: LayoutBuilder(
                builder: (context, constraints) {
                  _syncGeometry(constraints);
                  final frame = _frame;
                  if (frame == null) {
                    return const SizedBox.shrink();
                  }
                  final cell = _measureCell();
                  return CustomPaint(
                    size: Size(constraints.maxWidth, constraints.maxHeight),
                    painter: TerminalPainter(
                      frame: frame,
                      cellWidth: cell.width,
                      cellHeight: cell.height,
                      fontSize: _fontSize,
                    ),
                  );
                },
              ),
            ),
            if (banner != null)
              Positioned(
                top: 0,
                left: 0,
                right: 0,
                child: banner,
              ),
            Positioned(
              right: 10,
              bottom: 8,
              child: _PerfOverlay(
                fps: _fps,
                dropped: _dropped,
                perf: _perf,
              ),
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
    );
  }
}

class _PerfOverlay extends StatelessWidget {
  final int fps;
  final int dropped;
  final PerfStats? perf;

  const _PerfOverlay({required this.fps, required this.dropped, this.perf});

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
        if (perf != null) ...[
          Text(
            'pack ${perf!.packUsAvg}/${perf!.packUsMax}μs',
            style: style,
          ),
          Text(
            'render ${perf!.renderUsAvg}/${perf!.renderUsMax}μs',
            style: style,
          ),
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

  const _Banner({required this.icon, required this.text, this.error = false, this.onBack});

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
          trailing: onBack == null
              ? null
              : TextButton(onPressed: onBack, child: const Text('返回')),
        ),
      ),
    );
  }
}
