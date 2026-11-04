#!/usr/bin/env bash
# 编译校验：源码在 /mnt/d/pay2（Windows 可见），target 放 WSL 原生盘（避开 9p 慢盘）
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1
echo "=== cargo test -p ctp-core ==="
cargo test -p ctp-core 2>&1 | tail -80
echo "EXIT=$?"
