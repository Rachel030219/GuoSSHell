import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';

import 'src/bindings/bindings.dart';
import 'src/terminal/terminal_page.dart';

Future<void> main() async {
  await initializeRust(assignRustSignal);
  runApp(const GuoSSHellApp());
}

/// M1 迭代用的自动连接钩子（debug 构建 + --dart-define 注入，见 README）。
const _autoHost = String.fromEnvironment('GUOSH_HOST');
const _autoPort = int.fromEnvironment('GUOSH_PORT', defaultValue: 22);
const _autoUser = String.fromEnvironment('GUOSH_USER');
const _autoPass = String.fromEnvironment('GUOSH_PASS');

class GuoSSHellApp extends StatelessWidget {
  const GuoSSHellApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'GuoSSHell',
      theme: ThemeData(
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(
          seedColor: Colors.teal,
          brightness: Brightness.dark,
        ),
      ),
      home: TerminalPage(
        autoHost: _autoHost.isEmpty ? null : _autoHost,
        autoPort: _autoPort,
        autoUsername: _autoUser.isEmpty ? null : _autoUser,
        autoPassword: _autoPass.isEmpty ? null : _autoPass,
      ),
    );
  }
}
