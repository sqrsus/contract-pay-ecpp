#!/usr/bin/env bash
# 汇总测试结果（只取结果行，避免中文测试名在控制台编码错乱）
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1

OUT=/mnt/d/pay2/test-report.txt
SUMMARY=/mnt/d/pay2/test-summary.txt

cargo test --workspace > "$OUT" 2>&1
EXIT=$?

{
  echo "=== 结果汇总 ==="
  echo "EXIT=$EXIT"
  grep -E "^(test result|running|error|warning: unused)" "$OUT" || true
  echo "ok_cases=$(grep -c '^test .* ok$' "$OUT")"
} > "$SUMMARY"

cat "$SUMMARY"
