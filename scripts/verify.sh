#!/usr/bin/env bash
# 最终验收：干净重编译 + 全量测试（结果汇总写入仓库根 verify.txt）
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1

LOG=/mnt/d/pay2/verify.txt
BUILD_OUT=/tmp/ctp_build.out
TEST_OUT=/tmp/ctp_test.out
: > "$LOG"

echo "== 1) 编译（含警告检查） ==" >> "$LOG"
cargo build --workspace > "$BUILD_OUT" 2>&1
echo "build exit=$?" >> "$LOG"
grep -E "^(warning|error)" "$BUILD_OUT" >> "$LOG" || echo "no warning" >> "$LOG"

echo "== 2) 全量测试 ==" >> "$LOG"
cargo test --workspace > "$TEST_OUT" 2>&1
echo "test exit=$?" >> "$LOG"
grep -E "^test result|^error" "$TEST_OUT" >> "$LOG"

echo "== 3) 产物 ==" >> "$LOG"
ls -la "$CARGO_TARGET_DIR/debug/ctp-gui" >> "$LOG" 2>&1

cat "$LOG"
