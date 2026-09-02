#!/usr/bin/env bash
# 一键运行 liuhuo_core 独立端到端测试（非 cargo test，与源码隔离）。
# 用法：在仓库根目录执行  bash test_scripts/run_all.sh
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== [0/3] 构建核心库（cargo build）=="
cargo build --quiet

RLIB="$(ls -t target/debug/libliuhuo_core.rlib 2>/dev/null | head -1)"
if [ -z "$RLIB" ]; then
    echo "未找到核心库 rlib，构建失败"; exit 1
fi

echo "== [1/3] 编译独立测试脚本 =="
DEPS="$(ls target/debug/deps/libserde*.rlib target/debug/deps/libserde_json*.rlib 2>/dev/null | sort -u)"
EXTERN=""
for d in $DEPS; do
    base="$(basename "$d")"
    name="${base#lib}"; name="${name%%-*}"
    EXTERN="$EXTERN --extern ${name}=${d}"
done

rustc --edition 2024 \
    -L target/debug/deps \
    $EXTERN \
    --extern liuhuo_core="${RLIB}" \
    -o test_scripts/e2e_pipeline.exe \
    test_scripts/e2e_pipeline.rs

echo "== [2/3] 运行端到端测试 =="
./test_scripts/e2e_pipeline.exe
RC=$?

echo "== [3/3] 完成 =="
exit $RC
