#!/usr/bin/env bash
# 全工作区编译与测试
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1
echo "=== cargo build --workspace ==="
cargo build --workspace 2>&1 | tail -60
echo "=== cargo test --workspace ==="
cargo test --workspace 2>&1 | tail -80
