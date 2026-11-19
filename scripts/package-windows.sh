#!/usr/bin/env bash
# 打 Windows 安装包：release 构建 → NSIS 编译 → dist/*-setup.exe
#
# 为什么是 NSIS 而不是 Inno Setup / WiX：三者的安装器都能用，但只有 NSIS 编译
# 出来的安装器可以 RequestExecutionLevel user（见 installer/hipda-nsh.nsi 注释）——
# 装到 %LOCALAPPDATA%\Programs，全程不提权。NSIS 官方分发的还是**便携 zip**，
# 解包即用，不需要在机器上装任何东西。
#
# 用法：
#   scripts/package-windows.sh              # 默认先 cargo build --release
#   SKIP_BUILD=1 scripts/package-windows.sh # 复用现有 release 产物
#   NSIS=/path/to/makensis scripts/package-windows.sh

set -euo pipefail
cd "$(dirname "$0")/.."

# GNU target 需要 PATH 里有 mingw 的 dlltool / gcc，否则 build.rs 与依赖编译会失败
# shellcheck source=/dev/null
source scripts/env-win.sh

NSIS="${NSIS:-$HOME/tools/nsis/nsis-3.11/Bin/makensis.exe}"
if [ ! -x "$NSIS" ]; then
  echo "找不到 makensis：$NSIS" >&2
  echo "下载 NSIS 便携包（约 2 MB）解压即可，或用 NSIS=... 指定路径：" >&2
  echo "  https://master.dl.sourceforge.net/project/nsis/NSIS%203/3.11/nsis-3.11.zip" >&2
  exit 1
fi

# 版本号只在 nsh.json 里维护一份，图标/安装器都跟着它走
VERSION=$(python -c "import json,sys; print(json.load(open(sys.argv[1]))['version'])" nsh.json)
echo "==> 版本 $VERSION"

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "==> cargo build --release"
  cargo build --release --bin hipda-nsh
fi

mkdir -p dist
echo "==> NSIS 编译"
"$NSIS" -V2 -DAPP_VERSION="$VERSION" installer/hipda-nsh.nsi

echo "==> 产物"
ls -lh dist/*.exe
