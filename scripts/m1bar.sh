#!/usr/bin/env bash
# m1bar.sh —— M1 帧率视觉自检：每帧刷一次的进度条，60Hz × 10s = 600 帧刷完。
#
# 用法：
#   1) 上传到远端：  scp scripts/m1bar.sh user@host:/tmp/
#   2) 连接时执行：  GUOSH_CMD="bash /tmp/m1bar.sh"（或 chmod +x 后填 /tmp/m1bar.sh）
#
# 每帧 = 一次 \r 行首重画（绿→黄→红渐变 + 百分比 + 帧号），看两点：
#   * 条是否**顺滑**推进——卡顿/跳格就是掉帧；
#   * 结尾一行报实际刷新率——远低于 60 Hz 说明链路跟不上。
# 有 python3 走绝对节拍调度（精确 60Hz）；没有就用 bash + sleep 0.0166 兜底
# （略低于 60Hz，sleep 的 fork 开销计入）。两者结束都打印实测数字。

set -u

FPS=60
SECS=10
WIDTH=40
TOTAL=$((FPS * SECS))

if command -v python3 >/dev/null 2>&1; then
  exec python3 - "$FPS" "$SECS" "$WIDTH" <<'PYEOF'
import sys, time

fps, secs, width = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
total = fps * secs
start = time.monotonic()
for i in range(1, total + 1):
    pct = i * 100 // total
    filled = width * i // total
    color = "\033[32m" if pct < 50 else ("\033[33m" if pct < 80 else "\033[31m")
    bar = "█" * filled + "░" * (width - filled)
    sys.stdout.write(f"\r{color}{bar}\033[0m {pct:3d}%  frame {i}/{total}")
    sys.stdout.flush()
    # 绝对节拍：第 i 帧对齐 start + i/fps，不随单帧开销漂移
    delay = start + i / fps - time.monotonic()
    if delay > 0:
        time.sleep(delay)
elapsed = time.monotonic() - start
print(f"\n\033[1;32m完成\033[0m 用时 {elapsed:.2f}s · 实际 {total / elapsed:.1f} Hz（目标 {fps}）· {total} 帧")
PYEOF
  exit 0
fi

# ── bash 兜底路径（无 python3 的机器）─────────────────────────────────────
start_us=""
if [[ -n "${EPOCHREALTIME:-}" ]]; then
  start_us="${EPOCHREALTIME/./}" # 微秒整数（bash ≥ 5）
fi

bar=""
for ((i = 1; i <= TOTAL; i++)); do
  pct=$((i * 100 / TOTAL))
  filled=$((WIDTH * i / TOTAL))
  if ((pct < 50)); then
    color=$'\033[32m'
  elif ((pct < 80)); then
    color=$'\033[33m'
  else
    color=$'\033[31m'
  fi
  bar=""
  for ((j = 0; j < filled; j++)); do bar+="█"; done
  for ((j = filled; j < WIDTH; j++)); do bar+="░"; done
  printf '\r%s%s\033[0m %3d%%  frame %d/%d' "$color" "$bar" "$pct" "$i" "$TOTAL"
  sleep 0.0166
done

if [[ -n "$start_us" ]]; then
  end_us="${EPOCHREALTIME/./}"
  elapsed_us=$((end_us - start_us))
  hz=$((TOTAL * 1000000 / elapsed_us))
  printf '\n\033[1;32m完成\033[0m 用时 %d.%02ds · 实际 %d Hz（目标 %d）· %d 帧\n' \
    $((elapsed_us / 1000000)) $(((elapsed_us % 1000000) / 10000)) "$hz" "$FPS" "$TOTAL"
else
  printf '\n\033[1;32m完成\033[0m 用时约 %ds（秒级计时）· %d 帧\n' "$((SECONDS))" "$TOTAL"
fi
