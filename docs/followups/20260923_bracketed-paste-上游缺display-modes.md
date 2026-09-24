# bracketed paste 未实现（上游缺 display_modes 字段）

- 状态：待办（**已定为可能的第一个 fork 点**，见 PLAN §9 待定项 3）
- 记录日期：2026-09-23

## 现状

M2a 的粘贴是直发原文（当作键盘输入）。多行粘贴在 shell 里会逐行执行、
在 vim autoindent 下会缩进阶梯。要按远端模式正确包裹（DECSET 2004
的 `\e[200~ … \e[201~`），必须知道远端有没有开启括号粘贴模式：
alacritty 引擎内部有这个状态，但上游 rsHell 的
`TerminalDisplayModes` 没有暴露（只有 mouse_reporting / alternate_screen 等）。

## 下一步

按 PLAN §9 的 fork 规则改上游（不是无限期绕开）：
adapter 把 `TermMode::BRACKETED_PASTE` 映射成 `TerminalDisplayModes`
的新字段 `bracketed_paste`；我们读该字段，开启时粘贴包
`\e[200~/\e[201~`，未开启直发原文。fork 后在 `rust/UPSTREAM.md`
记录改了哪几行、为什么。
