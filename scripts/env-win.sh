# Windows (Git Bash) 本地构建环境 —— 用法：`. scripts/env-win.sh`
#
# 为什么需要这个文件：本机没有 MSVC 编译器，Rust 使用的是 GNU 工具链
# （x86_64-pc-windows-gnu）。GNU target 在链接/编译期需要 dlltool 与 gcc，
# 而 rustup 自带的 rust-mingw 不含这两个程序，所以把 mingw-w64 工具链
# 放在 ~/tools/mingw64 并临时加入 PATH，避免污染系统环境变量。
#
# 三条依赖的来源（均取境内镜像，见 ~/tools/pkgs/fetch_mingw.py）：
#   ~/.cargo/bin                        rustup（rsproxy.cn 安装）
#   ~/tools/mingw64/mingw64/bin         gcc 16.2 + binutils 2.47（MSYS2 包，清华镜像）
#   ~/.cargo/config.toml                crates.io 走 rsproxy 稀疏索引

export PATH="$HOME/tools/mingw64/mingw64/bin:$HOME/.cargo/bin:$PATH"

# 中文字体/emoji 相关警告在 GNU target 下无影响，这里不额外配置
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-12}"
