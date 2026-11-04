#!/usr/bin/env bash
# 端到端业务演练（无界面）。
# 命令行入口已移除，业务链路由 crates/ctp-app/tests/consistency.rs 的集成测试承担：
# 建库 → 演示数据 → 台账/分类/期间三表口径 → 超付拦截 → 审计价切换 → 作废回退 → 六项自检。
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1

OUT=/mnt/d/pay2/build-log.txt
: > "$OUT"

{
  echo "=== 端到端业务演练 ==="
  echo "覆盖的断言："
} >> "$OUT"

cargo test -p ctp-app --test consistency -- --list >> "$OUT" 2>&1

echo >> "$OUT"
echo "=== 执行 ===" >> "$OUT"
cargo test -p ctp-app --test consistency -- --nocapture >> "$OUT" 2>&1
echo "[exit=$?]" >> "$OUT"

tail -30 "$OUT"
