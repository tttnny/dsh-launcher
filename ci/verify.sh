#!/usr/bin/env bash
# 本仓库合并前质量门禁的唯一入口。
#
# 为什么要有这个脚本：这些检查原先散落在四处——AGENTS.md 日常开发那三行、CI 两个
# job 各自的步骤表、package.json 里一串 test:* 脚本，剩下的只活在人的记忆里。四份
# 清单必然各自漂移，而漂移的代价是「CI 里没接上的检查，本地也没人跑」。现在清单只
# 有这一份，CI 与每个人执行的是同一行命令。
#
# `ci/*.test.mjs` 一律按通配符发现，不写死名单：一个新增的测试文件如果还得有人记得
# 往清单里补一行，它就已经在「被漏跑」的路上了——这正是要结束的那种漂移。
#
# 用法: ci/verify.sh [--quick | --source-only]
#
#   （无参数）     全部检查。合并前跑这一次。
#   --quick       只跑 clippy 与 Rust 测试。合并之间快速过一次用。
#   --source-only 只跑版本同步检查与 ci/*.test.mjs。这一类只读文件、不需要
#                 node_modules，也不需要 pnpm 与 Rust 工具链，是 Linux CI job
#                 （它刻意不装任何依赖）能跑的那一份。
#
# 需要的工具：无参数要 cargo（含 rustfmt 与 clippy 组件）、node、pnpm；
#             --quick 要 cargo（含 clippy）；--source-only 只要 node。
#
# 任何一步失败立即以非零退出码结束，并说出是哪一步失败。
set -euo pipefail

cd "$(dirname "$0")/.."

MANIFEST="src-tauri/Cargo.toml"

usage() {
  cat <<'TEXT'
用法: ci/verify.sh [--quick | --source-only]

  （无参数）     全部检查：cargo fmt --check、clippy、Rust 测试、版本同步、
                 ci/*.test.mjs（通配符发现）、pnpm build。合并前跑这一次。
  --quick       只跑 clippy 与 Rust 测试。合并之间快速过一次用。
  --source-only 只跑版本同步检查与 ci/*.test.mjs，不需要 node_modules、
                 pnpm 与 Rust 工具链（Linux CI job 跑的就是这一份）。
TEXT
}

MODE="full"
case "${1:-}" in
  "") ;;
  --quick) MODE="quick" ;;
  --source-only) MODE="source_only" ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "错误：未知参数 '$1'。" >&2
    echo >&2
    usage >&2
    exit 2
    ;;
esac

# 依赖是按模式要的：--source-only 在 CI 的 Linux job 里跑，那里既没有 pnpm 也没有
# Rust，把不需要的东西也 check 一遍只会让那个 job 永远起不来。
require() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "错误：找不到 ${1}（${2}）。" >&2
    exit 1
  }
}

case "$MODE" in
  full)
    require cargo "装 Rust 工具链：https://rustup.rs；还需要 rustfmt 与 clippy 组件"
    require node "装 Node.js"
    require pnpm "corepack enable 之后即可使用仓库里的 pnpm"
    ;;
  quick)
    require cargo "装 Rust 工具链：https://rustup.rs；还需要 clippy 组件"
    ;;
  source_only)
    require node "装 Node.js"
    ;;
esac

# 测试文件由通配符发现；一个都匹配不上说明这里的目录结构已经不是脚本以为的样子，
# 与其「零个测试通过」，不如直接说落空了。
SUITES=(ci/*.test.mjs)
if [ ! -e "${SUITES[0]}" ]; then
  echo "错误：ci/*.test.mjs 一个都没匹配到。" >&2
  exit 1
fi

case "$MODE" in
  full) TOTAL=$((4 + ${#SUITES[@]} + 1)) ;;
  quick) TOTAL=2 ;;
  source_only) TOTAL=$((1 + ${#SUITES[@]})) ;;
esac

STEPS=0
step() {
  STEPS=$((STEPS + 1))
  printf '\n──── [%d/%d] %s\n' "$STEPS" "$TOTAL" "$1"
  printf '     $ %s\n' "${*:2}"
  local status=0
  "${@:2}" || status=$?
  if [ "$status" -ne 0 ]; then
    printf '\n错误：第 %d 步「%s」失败（退出码 %d）。\n' "$STEPS" "$1" "$status" >&2
    exit 1
  fi
}

run_suites() {
  local suite
  for suite in "${SUITES[@]}"; do
    step "源码级测试 ${suite}" node --test "$suite"
  done
}

case "$MODE" in
  quick)
    echo "verify: 模式 ${MODE}，共 ${TOTAL} 步"
    ;;
  *)
    echo "verify: 模式 ${MODE}，共 ${TOTAL} 步（${#SUITES[@]} 个 ci/*.test.mjs 由通配符发现）"
    ;;
esac

case "$MODE" in
  quick)
    step "Clippy（warning 即错）" \
      cargo clippy --manifest-path "$MANIFEST" --workspace --all-targets --locked -- -D warnings
    step "Rust 测试" \
      cargo test --manifest-path "$MANIFEST" --workspace --locked
    ;;
  source_only)
    step "版本同步（package.json / Cargo.toml / tauri.conf.json）" \
      node ci/check-versions.mjs
    run_suites
    ;;
  full)
    step "Rust 格式检查（cargo fmt --check）" \
      cargo fmt --manifest-path "$MANIFEST" --all -- --check
    step "Clippy（warning 即错）" \
      cargo clippy --manifest-path "$MANIFEST" --workspace --all-targets --locked -- -D warnings
    step "Rust 测试" \
      cargo test --manifest-path "$MANIFEST" --workspace --locked
    step "版本同步（package.json / Cargo.toml / tauri.conf.json）" \
      node ci/check-versions.mjs
    run_suites
    step "前端构建（vue-tsc --noEmit && vite build）" \
      pnpm build
    ;;
esac

printf '\n全部通过：%d 步，用时 %d 秒。\n' "$STEPS" "$SECONDS"
