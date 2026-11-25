#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

BUNDLE="src-tauri/target/release/bundle/macos/dsh-launcher.app"
DEST="/Applications/dsh-launcher.app"

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

pkill -f "dsh-launcher.app" 2>/dev/null || true
sleep 1
rm -rf "$DEST"
ditto "$BUNDLE" "$DEST"
codesign --verify --deep --strict "$DEST"

echo
echo "已安装: $DEST"
echo "Designated requirement:"
echo "  $DR"
