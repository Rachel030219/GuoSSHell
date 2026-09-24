import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/frame.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:terminal_view/terminal_view.dart';

FrameRun run(int start, int len, String text, {int attrs = 0}) => FrameRun(
      start: start,
      len: len,
      attrs: attrs,
      fg: const DefaultColor(),
      bg: const DefaultColor(),
      text: text,
    );

TerminalFrame frame({
  int cols = 20,
  int rows = 3,
  required List<FrameRow> lines,
  int cursorCol = -1,
  int cursorRow = -1,
}) =>
    TerminalFrame(
      cols: cols,
      rows: rows,
      cursorCol: cursorCol,
      cursorRow: cursorRow,
      lines: lines,
    );

void main() {
  FrameRow rowAt(int stableRow, String text) => FrameRow(
        stableRow: stableRow,
        wrapped: false,
        runs: [run(0, text.length, text)],
      );

  test('同一行的内容跨帧不变则复用同一对象（锚点与 Picture 缓存的地基）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      rowAt(10, 'aaaa'),
      rowAt(11, 'bbbb'),
      rowAt(12, 'cccc'),
    ]));
    final first = [0, 1, 2].map(terminal.lineAt).toList();

    terminal.applyFrame(frame(lines: [
      rowAt(10, 'aaaa'),
      rowAt(11, 'bbbb'),
      rowAt(12, 'cccc'),
    ]));
    for (var i = 0; i < 3; i++) {
      expect(identical(first[i], terminal.lineAt(i)), isTrue,
          reason: '同 stable_row 同内容的行必须复用对象');
    }
  });

  test('内容滚走后行对象跟着内容走（选区锚点才不会停在原位）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      rowAt(10, 'top'),
      rowAt(11, 'mid'),
      rowAt(12, 'bot'),
    ]));
    final midLine = terminal.lineAt(1);

    // 内容整体上移一行：mid（stable_row 11）应出现在位置 0。
    terminal.applyFrame(frame(lines: [
      rowAt(11, 'mid'),
      rowAt(12, 'bot'),
      rowAt(13, 'new'),
    ]));
    expect(identical(terminal.lineAt(0), midLine), isTrue);
  });

  test('stableRow 换算：视口行 ↔ 引擎绝对行', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      rowAt(10, 'aaaa'),
      rowAt(11, 'bbbb'),
      rowAt(12, 'cccc'),
    ]));
    expect(terminal.stableRowAt(0), 10);
    expect(terminal.stableRowAt(2), 12);
    expect(terminal.stableRowAt(3), isNull);
    expect(terminal.viewportRowForStable(11), 1);
    expect(terminal.viewportRowForStable(99), isNull);

    // 内容整体上移：换算表跟着新帧更新。
    terminal.applyFrame(frame(lines: [
      rowAt(11, 'bbbb'),
      rowAt(12, 'cccc'),
      rowAt(13, 'dddd'),
    ]));
    expect(terminal.stableRowAt(0), 11);
    expect(terminal.viewportRowForStable(11), 0);
    expect(terminal.viewportRowForStable(10), isNull);
  });

  test('锚点已收养（attached），选区依赖这个性质', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 4, 'abcd')]),
    ]));
    final anchor = terminal.createAnchor(2, 0);
    expect(anchor.attached, isTrue);
    expect(anchor.offset, const CellOffset(2, 0));
  });

  test('getWordBoundary 按分隔符切词', () {
    final terminal = FrameTerminal();
    // "ls -la /tmp"：'-la' 的 '-' 在 x=3。
    terminal.applyFrame(frame(lines: [
      FrameRow(
        stableRow: 0,
        wrapped: false,
        runs: [run(0, 11, 'ls -la /tmp')],
      ),
    ]));
    // 词边界端点排他（fork 约定）：x=4 的 'l' → 词是 'la'。
    final word = terminal.getWordBoundary(const CellOffset(4, 0));
    expect(word, isNotNull);
    expect(terminal.getText(word), 'la');
    // 分隔符上没有词：返回 null，页面侧回退「单格选区」。
    expect(terminal.getWordBoundary(const CellOffset(3, 0)), isNull);
  });

  test('getText 尊重 wrapped 标志：软换行的物理行不插换行符', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 20, 'a' * 20)]),
      FrameRow(stableRow: 1, wrapped: true, runs: [run(0, 3, 'bbb')]),
      FrameRow(stableRow: 2, wrapped: false, runs: [run(0, 3, 'end')]),
    ]));
    // 端点排他：取满 'end' 要指到 x=3。
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(3, 2)),
    );
    expect(text, '${'a' * 20}bbb\nend');
  });

  test('帧变矮后高度跟着收敛（旧行尾巴不算进缓冲）', () {
    FrameRow row(int i) =>
        FrameRow(stableRow: i, wrapped: false, runs: [run(0, 3, 'r$i')]);
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(
      cols: 20,
      rows: 46,
      lines: List.generate(46, row),
    ));
    expect(terminal.height, 46);

    // 键盘弹出：帧变 25 行。缓冲物理长度仍是 46（fork 没有删行 API），
    // 但 height 必须收敛到 25，否则旧行会被当成缓冲内容（实机重影 bug）。
    terminal.applyFrame(frame(
      cols: 20,
      rows: 25,
      lines: List.generate(25, row),
    ));
    expect(terminal.height, 25);
    expect(terminal.lineAt(24), isNotNull);
  });

  test('复制时裁掉每行行尾空格（上游把空白格发成字面空格）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(
      cols: 20,
      rows: 2,
      lines: [
        FrameRow(
          stableRow: 0,
          wrapped: false,
          runs: [run(0, 20, 'abc${' ' * 17}')],
        ),
        FrameRow(
          stableRow: 1,
          wrapped: false,
          runs: [run(0, 20, 'de${' ' * 18}')],
        ),
      ],
    ));
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(20, 1)),
    );
    expect(text, 'abc\nde');
  });

  test('getText 裁掉行尾空白', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(frame(lines: [
      FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 4, 'abc')]),
    ]));
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(19, 0)),
    );
    expect(text, 'abc');
  });
}
