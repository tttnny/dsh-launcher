#!/usr/bin/env bash
# 本机构建 + 安装自测用的 .app。
#
# 与 `pnpm tauri build` 的区别是签名方式：这里用钥匙串里的 Apple Development 证书签名，
# 那条命令打出的 dmg 是未签名的原始产物。
#
# 为什么必须用证书签名：ad-hoc 签名的 designated requirement 就是 cdhash，
# App 内容一变 cdhash 就变，macOS 存的 TCC 授权（如「完全磁盘访问」）随之失配，
# 表现为系统设置里开关还开着、实际却没有权限。证书签名的 DR 只钉 bundle id 和
# 证书，不含 cdhash，重建后逐字节不变，授权得以跨更新保留。
#
# 用法: ci/build-local-app.sh [--no-build]
set -euo pipefail

cd "$(dirname "$0")/.."

BUNDLE="src-tauri/target/release/bundle/macos/dsh-launcher.app"
DEST="/Applications/dsh-launcher.app"

# 从钥匙串动态取身份，不硬编码：换证书后自动跟随，也不会把 Apple ID 写进公开仓库
SIGN_ID="$(security find-identity -v -p codesigning \
  | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
if [ -z "$SIGN_ID" ]; then
  echo "错误：钥匙串里找不到 Apple Development 证书。" >&2
  echo "用 Xcode → Settings → Accounts 登录 Apple ID 即可生成（免费账号即可）。" >&2
  exit 1
fi
echo "签名身份: $SIGN_ID"

if [ "${1:-}" != "--no-build" ]; then
  APPLE_SIGNING_IDENTITY="$SIGN_ID" pnpm tauri build --bundles app
elif [ ! -d "$BUNDLE" ]; then
  echo "错误：$BUNDLE 不存在，去掉 --no-build 先构建一次。" >&2
  exit 1
fi

# 产物必须真的由该证书签名，否则自测会静默退回 adhoc，授权又会在下次重建时失效。
# （Authority 行只有 --verbose 级别才输出）
signed_by() {
  codesign -dv --verbose=4 "$1" 2>&1 | sed -n 's/^Authority=//p' | head -1
}
ACTUAL="$(signed_by "$BUNDLE")"
if [ "$ACTUAL" != "$SIGN_ID" ]; then
  echo "产物当前签名身份为 '${ACTUAL:-<无>}'，补签为 $SIGN_ID"
  codesign --force --deep --sign "$SIGN_ID" "$BUNDLE"
  ACTUAL="$(signed_by "$BUNDLE")"
  if [ "$ACTUAL" != "$SIGN_ID" ]; then
    echo "错误：补签后签名身份是 '${ACTUAL:-<无>}'，期望 '$SIGN_ID'。" >&2
    exit 1
  fi
fi

DR="$(codesign -d -r- "$BUNDLE" 2>&1 | sed -n 's/^#* *designated => //p')"
case "$DR" in
  *cdhash*)
    echo "错误：DR 仍含 cdhash，TCC 授权会随重建失效：" >&2
    echo "  $DR" >&2
    exit 1
    ;;
esac

# 替换自测副本前先退出运行中的实例，避免删掉正在运行的 App
pkill -f "dsh-launcher.app" 2>/dev/null || true
sleep 1
rm -rf "$DEST"
ditto "$BUNDLE" "$DEST"
codesign --verify --deep --strict "$DEST"

echo
echo "已安装: $DEST"
echo "Designated requirement:"
echo "  $DR"
