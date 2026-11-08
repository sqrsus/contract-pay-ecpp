#!/usr/bin/env bash
# 最终验收：干净重编译 + 全量测试（结果汇总写入仓库根 verify.txt）
# R-39：严格失败传播。任一阶段失败，脚本最终以非 0 退出，不再恒返 0。
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1

LOG=/mnt/d/pay2/verify.txt
BUILD_OUT=/tmp/ctp_build.out
TEST_OUT=/tmp/ctp_test.out
: > "$LOG"

echo "== 1) 编译（含警告检查） ==" >> "$LOG"
if cargo build --workspace > "$BUILD_OUT" 2>&1; then
  BUILD_EXIT=0
else
  BUILD_EXIT=$?
fi
echo "build exit=$BUILD_EXIT" >> "$LOG"
grep -E "^(warning|error)" "$BUILD_OUT" >> "$LOG" || echo "no warning" >> "$LOG"

echo "== 2) 全量测试 ==" >> "$LOG"
if cargo test --workspace > "$TEST_OUT" 2>&1; then
  TEST_EXIT=0
else
  TEST_EXIT=$?
fi
echo "test exit=$TEST_EXIT" >> "$LOG"
grep -E "^test result|^error" "$TEST_OUT" >> "$LOG" || true

echo "== 3) 产物 ==" >> "$LOG"
if ls -la "$CARGO_TARGET_DIR/debug/ctp-gui" >> "$LOG" 2>&1; then
  ARTIFACT_EXIT=0
else
  ARTIFACT_EXIT=1
fi

cat "$LOG"

# 按阶段退出码返回：编译 / 测试 / 产物任一失败即整体失败
if [ "$BUILD_EXIT" -ne 0 ] || [ "$TEST_EXIT" -ne 0 ] || [ "$ARTIFACT_EXIT" -ne 0 ]; then
  exit 1
fi
exit 0
