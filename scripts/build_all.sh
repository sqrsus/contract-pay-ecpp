#!/usr/bin/env bash
# 全工作区编译与测试
# R-39：set -e + pipefail，cargo 失败会传播给调用方（旧版只 set -uo pipefail，恒返 0）。
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2 || exit 1
echo "=== cargo build --workspace ==="
cargo build --workspace 2>&1 | tail -60
echo "=== cargo test --workspace ==="
cargo test --workspace 2>&1 | tail -80
