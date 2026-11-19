//! 把 `assets/icon.ico` 编进 Windows 可执行文件，作为「程序图标」。
//!
//! 为什么要这个构建脚本：`nsh-platform-windows` 注册窗口类时用
//! `LoadIconW(instance(), PCWSTR(1))` 取 **exe 资源里 ID 为 1 的图标**
//! （window.rs:167），取不到就让窗口退回系统默认图标；资源管理器/任务栏固定
//! 看到的文件图标也来自同一处。NOTE: 运行时的 `AppOptions.icon` 帮不上忙——
//! `window::set_icon` 开头就 `filter(|_| !has_own_icon())`，exe 自带图标时它
//! 直接不生效。所以要做成「程序图标」只能把资源嵌进 exe。
//!
//! 找不到资源编译器时只打印 warning、不报错：图标缺失不影响功能，而换工具链
//! （GNU / MSVC）或者换机器时不该把编译整个打断。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // macOS / Linux 的 AppOptions 没有 icon 字段，图标由 bundle 决定，这里跳过
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let assets = manifest.join("assets");
    let rc = assets.join("icon.rc");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let gnu = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");

    // GNU 用 windres 出 COFF 目标文件，MSVC 用 rc.exe 出 .res；两者都能直接
    // 交给链接器（cargo:rustc-link-arg）
    let (program, args, artifact) = if gnu {
        let obj = out_dir.join("icon.o");
        (
            "windres",
            vec![
                "--include-dir".to_string(),
                assets.display().to_string(),
                "-O".to_string(),
                "coff".to_string(),
                "-o".to_string(),
                obj.display().to_string(),
                rc.display().to_string(),
            ],
            obj,
        )
    } else {
        let res = out_dir.join("icon.res");
        (
            "rc",
            vec![
                format!("/fo{}", res.display()),
                format!("/I{}", assets.display()),
                rc.display().to_string(),
            ],
            res,
        )
    };

    match Command::new(program).args(&args).status() {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg={}", artifact.display());
        }
        Ok(status) => println!(
            "cargo:warning={program} 退出码 {status}，exe 不会带程序图标（窗口将用系统默认图标）"
        ),
        Err(err) => println!(
            "cargo:warning=找不到 {program}（{err}），exe 不会带程序图标；装 mingw-w64 binutils 或 VS C++ 工具链后重编即可"
        ),
    }
}
