#!/usr/bin/env bash
# 打 macOS 安装包：通用架构 .app → 带 Applications 快捷方式的 .dmg
#
# 用法：scripts/package-macos.sh [版本号]
# 产物：dist/HiPDA-nsh-<版本>-macos-universal.dmg
#
# 需要在 macOS 上跑，且已装 rustup 目标：
#   rustup target add x86_64-apple-darwin   # Apple silicon 机器上打通用包必须
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-0.1.0}"
APP_NAME="HiPDA·nsh 版"
OUT_DIR=".nsh/out"
DMG="dist/HiPDA-nsh-${VERSION}-macos-universal.dmg"

echo "==> 编译并打包 ${APP_NAME}（arm64 + x86_64）"
nsh package --target macos --arch universal

if ! lipo -info "$OUT_DIR/${APP_NAME}.app/Contents/MacOS/HiPDAnsh" | grep -q "x86_64"; then
  echo "!! 产物不是通用二进制，检查是否漏装 x86_64-apple-darwin 目标" >&2
  exit 1
fi

echo "==> 生成 DMG（含 /Applications 快捷方式）"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$OUT_DIR/${APP_NAME}.app" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

mkdir -p dist
rm -f "$DMG"
hdiutil create -volname "HiPDA nsh ${VERSION}" -srcfolder "$STAGE" \
  -ov -format UDZO -quiet "$DMG"

echo "==> 完成：$DMG ($(du -h "$DMG" | cut -f1))"
echo "    注意：本包是 ad-hoc 签名，别的机器首次打开需右键 →「打开」，"
echo "    或执行 xattr -dr com.apple.quarantine \"/Applications/$APP_NAME.app\""
