import 'dart:async';

import 'package:flutter/material.dart';
import 'package:terminal_view/terminal_view.dart' show TerminalKey;

import 'frame_terminal.dart';

/// M2 键位条：双排固定布局。
///
/// * 修饰键（Ctrl/Alt）：点按挂住一次、长按锁定、锁定后再点解除——
///   状态存于 [FrameTerminal]，软键盘的下一个按键同样带得上修饰键。
/// * 其余按键：短按抬起发一次；长按（Flutter 手势识别器默认阈值）触发时
///   先发一个、之后按 [_keyRepeatInterval] 自动重复，抬起即停。
/// 用户自定义排布留给设置体系（M3+）。
class TerminalKeyBar extends StatefulWidget {
  final FrameTerminal terminal;

  const TerminalKeyBar({super.key, required this.terminal});

  @override
  State<TerminalKeyBar> createState() => _TerminalKeyBarState();
}

class _TerminalKeyBarState extends State<TerminalKeyBar> {
  static const List<TerminalKey> _functionRow = [
    TerminalKey.escape,
    TerminalKey.tab,
    TerminalKey.arrowUp,
    TerminalKey.arrowDown,
    TerminalKey.arrowLeft,
    TerminalKey.arrowRight,
  ];

  static const List<String> _symbolRow = ['|', '/', '-', '~', '.'];

  /// 长按自动重复的间隔。长按的触发阈值不在这里写死——
  /// 用 GestureDetector 长按识别器的默认时长。
  static const Duration _keyRepeatInterval = Duration(milliseconds: 200);

  Timer? _repeatTimer;
  String? _pressedId; // 手指按下的键帽（按下即高亮）
  String? _repeatingId; // 正在自动重复的键帽

  bool _isLit(String id) => _pressedId == id || _repeatingId == id;

  void _down(String id) {
    setState(() => _pressedId = id);
  }

  /// 短按抬起：发一次。长按获胜时 onTapUp 不会触发，走 [_endRepeat]。
  void _up(String id, void Function() send) {
    if (_pressedId != id) return;
    _pressedId = null;
    setState(() {});
    send();
  }

  /// 长按触发：立即发一个，之后按 [_keyRepeatInterval] 重复。
  void _longPress(String id, void Function() send) {
    if (_repeatingId == id) return;
    _repeatingId = id;
    setState(() {});
    send();
    _repeatTimer = Timer.periodic(_keyRepeatInterval, (_) => send());
  }

  /// 抬起 / 取消：停止重复。
  void _endRepeat(String id) {
    if (_repeatingId != id && _pressedId != id) return;
    _repeatTimer?.cancel();
    _repeatTimer = null;
    _repeatingId = null;
    _pressedId = null;
    setState(() {});
  }

  @override
  void dispose() {
    _repeatTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      color: scheme.surface.withValues(alpha: 0.6),
      child: SafeArea(
        top: false,
        child: ListenableBuilder(
          listenable: widget.terminal,
          builder: (context, _) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _buildRow([
                for (final key in _functionRow)
                  _cap(
                    context,
                    scheme,
                    'key:${key.name}',
                    48,
                    () => widget.terminal.keyInput(key),
                    Text(_label(key), style: _capStyle(scheme)),
                  ),
                _modifierCap(context, scheme, 'ctrl', 'Ctrl'),
                _modifierCap(context, scheme, 'alt', 'Alt'),
                _cap(
                  context,
                  scheme,
                  'key:backspace',
                  40,
                  () => widget.terminal.keyInput(TerminalKey.backspace),
                  Text('⌫', style: _capStyle(scheme)),
                ),
              ]),
              _buildRow([
                for (final char in _symbolRow)
                  _cap(
                    context,
                    scheme,
                    'sym:$char',
                    40,
                    () => widget.terminal.textInput(char),
                    Text(
                      char,
                      style:
                          _capStyle(scheme, fontFamily: 'Menlo', fontSize: 13),
                    ),
                  ),
              ]),
            ],
          ),
        ),
      ),
    );
  }

  TextStyle _capStyle(
    ColorScheme scheme, {
    String? fontFamily,
    double fontSize = 12,
  }) =>
      TextStyle(
          fontSize: fontSize, fontFamily: fontFamily, color: scheme.onSurface);

  /// Wrap 而不是 Row+Spacer：窄屏（iPhone 13 mini 级别）自动换行，
  /// 不会溢出；也不用把 Spacer 塞进 Padding（ParentData 会炸）。
  Widget _buildRow(List<Widget> children) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
      child: Wrap(spacing: 4, runSpacing: 2, children: children),
    );
  }

  /// 通用按键帽：短按抬起发一次；长按触发后自动重复。
  /// 固定宽度 + alignment 居中——不能用「minWidth + alignment」的组合，
  /// 带 alignment 的 Container 在有界约束下会撑满可用宽度（iPad 上一个
  /// 键帽占一整行就是这么来的）。
  Widget _cap(
    BuildContext context,
    ColorScheme scheme,
    String id,
    double width,
    void Function() send,
    Widget child,
  ) {
    final lit = _isLit(id);
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTapDown: (_) => _down(id),
      onTapUp: (_) => _up(id, send),
      onTapCancel: () => _endRepeat(id),
      onLongPressStart: (_) => _longPress(id, send),
      onLongPressEnd: (_) => _endRepeat(id),
      onLongPressCancel: () => _endRepeat(id),
      child: Container(
        width: width,
        height: 34,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          border: Border.all(color: scheme.outline),
          borderRadius: BorderRadius.circular(17),
          color: lit ? scheme.primaryContainer : null,
        ),
        child: child,
      ),
    );
  }

  String _label(TerminalKey key) => switch (key) {
        TerminalKey.escape => 'Esc',
        TerminalKey.tab => 'Tab',
        TerminalKey.arrowUp => '↑',
        TerminalKey.arrowDown => '↓',
        TerminalKey.arrowLeft => '←',
        TerminalKey.arrowRight => '→',
        _ => key.name,
      };

  /// 修饰键帽：点按循环（挂住 → 锁定 → 解除），长按直接锁定。
  /// 修饰键不参与自动重复。
  Widget _modifierCap(
    BuildContext context,
    ColorScheme scheme,
    String modifier,
    String label,
  ) {
    final locked = widget.terminal.isModifierLocked(modifier);
    final latched = widget.terminal.isModifierLatched(modifier);
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => widget.terminal.tapModifier(modifier),
      onLongPressStart: (_) => widget.terminal.lockModifier(modifier),
      child: Container(
        width: 56,
        height: 34,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          border: Border.all(color: scheme.outline),
          borderRadius: BorderRadius.circular(17),
          color: locked
              ? scheme.primary
              : latched
                  ? scheme.primaryContainer
                  : null,
        ),
        child: Text(
          locked ? '$label 🔒' : label,
          style: TextStyle(
            fontSize: 12,
            color: locked ? scheme.onPrimary : scheme.onSurface,
          ),
        ),
      ),
    );
  }
}
