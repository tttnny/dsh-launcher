#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

MANIFEST="src-tauri/Cargo.toml"

usage() {
  cat <<'TEXT'
用法: scripts/verify.sh [--quick]

  （无参数）     全部检查：cargo fmt --check、clippy、Rust 测试、版本同步、
                 scripts/*.test.mjs（通配符发现）、注释基线、prettier --check、pnpm build。
                 合并前跑这一次。
  --quick       只跑 clippy 与 Rust 测试。合并之间快速过一次用。
TEXT
}

MODE="full"
case "${1:-}" in
  "") ;;
  --quick) MODE="quick" ;;
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
esac

SUITES=(scripts/*.test.mjs)
if [ ! -e "${SUITES[0]}" ]; then
  SUITES=()
fi

case "$MODE" in
  full) TOTAL=$((6 + ${#SUITES[@]} + 1)) ;;
  quick) TOTAL=2 ;;
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
  if [ "${#SUITES[@]}" -eq 0 ]; then
    return 0
  fi
  for suite in "${SUITES[@]}"; do
    step "源码级测试 ${suite}" node --test "$suite"
  done
}

case "$MODE" in
  quick)
    echo "verify: 模式 ${MODE}，共 ${TOTAL} 步"
    ;;
  *)
    echo "verify: 模式 ${MODE}，共 ${TOTAL} 步（${#SUITES[@]} 个 scripts/*.test.mjs 由通配符发现）"
    ;;
esac

case "$MODE" in
  quick)
    step "Clippy（warning 即错）" \
      cargo clippy --manifest-path "$MANIFEST" --workspace --all-targets --locked -- -D warnings
    step "Rust 测试" \
      cargo test --manifest-path "$MANIFEST" --workspace --locked
    ;;
  full)
    step "Rust 格式检查（cargo fmt --check）" \
      cargo fmt --manifest-path "$MANIFEST" --all -- --check
    step "Clippy（warning 即错）" \
      cargo clippy --manifest-path "$MANIFEST" --workspace --all-targets --locked -- -D warnings
    step "Rust 测试" \
      cargo test --manifest-path "$MANIFEST" --workspace --locked
    step "版本同步（package.json / Cargo.toml / tauri.conf.json）" \
      node scripts/check-versions.mjs
    step "注释基线（禁止新增注释）" \
      node scripts/check-comments.mjs
    run_suites
    step "前端格式检查（prettier --check）" \
      pnpm exec prettier --check .
    step "前端构建（vue-tsc --noEmit && vite build）" \
      pnpm build
    ;;
esac

printf '\n全部通过：%d 步，用时 %d 秒。\n' "$STEPS" "$SECONDS"
