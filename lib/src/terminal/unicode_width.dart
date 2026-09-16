/// 单个 code point 的终端列宽（展示层的纯函数，无状态）。
///
/// 覆盖 M1 验收需要的范围：CJK/全角/Hangul 按 2 列，组合记号与
/// 变体选择符按 0 列（贴附前一个字形），其余按 1 列。
/// 完整的 EastAsianWidth 表（以及 grapheme cluster/ZWJ 序列）是
/// terminal_view fork 的活，这里先立一个够用的子集。
int runeCellWidth(int rune) {
  // 组合记号 / 零宽字符 / 变体选择符 / ZWJ
  if ((rune >= 0x0300 && rune <= 0x036F) ||
      (rune >= 0x200B && rune <= 0x200F) ||
      rune == 0x200D ||
      (rune >= 0xFE00 && rune <= 0xFE0F) ||
      rune == 0x20E3 ||
      (rune >= 0xE0100 && rune <= 0xE01EF)) {
    return 0;
  }
  // East Asian Wide / Fullwidth（主要区段）
  if ((rune >= 0x1100 && rune <= 0x115F) || // Hangul Jamo
      (rune >= 0x2E80 && rune <= 0x303E) || // CJK 部首、符号
      (rune >= 0x3041 && rune <= 0x33FF) || // 假名、注音、兼容
      (rune >= 0x3400 && rune <= 0x4DBF) || // CJK 扩展 A
      (rune >= 0x4E00 && rune <= 0x9FFF) || // CJK 统一
      (rune >= 0xA000 && rune <= 0xA4CF) || // 彝文
      (rune >= 0xAC00 && rune <= 0xD7A3) || // Hangul 音节
      (rune >= 0xF900 && rune <= 0xFAFF) || // CJK 兼容
      (rune >= 0xFE30 && rune <= 0xFE4F) || // CJK 兼容形式
      (rune >= 0xFF00 && rune <= 0xFF60) || // 全角形式
      (rune >= 0xFFE0 && rune <= 0xFFE6) ||
      (rune >= 0x1F300 && rune <= 0x1F64F) || // emoji 主区
      (rune >= 0x1F900 && rune <= 0x1F9FF) || // emoji 补充
      (rune >= 0x1FA70 && rune <= 0x1FAFF) ||
      (rune >= 0x20000 && rune <= 0x2FFFD) || // CJK 扩展 B-F
      (rune >= 0x30000 && rune <= 0x3FFFD)) {
    return 2;
  }
  return 1;
}
