#!/usr/bin/env bash
# 编译校验：源码在 /mnt/d/pay2（Windows 可见），target 放 WSL 原生盘（避开 9p 慢盘）
# R-39：pipefail + 真实退出码传播，测试失败不再被 tail 掩掉。
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1
echo "=== cargo test -p ctp-core ==="
if cargo test -p ctp-core 2>&1 | tail -80; then
  EXIT=0
else
  EXIT=$?
fi
echo "EXIT=$EXIT"
exit "$EXIT"
