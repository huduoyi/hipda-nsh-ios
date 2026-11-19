#!/usr/bin/env python3
"""把 NSH 平台 cookie jar 里的论坛 cookie 导出成 `verify_writes` 能用的格式。

App 的登录态由 NSH 的网络栈维护，URLSession 会把 cookie 落到
`~/Library/HTTPStorages/<process>.binarycookies`。这里解析该二进制格式，
只取论坛需要的几个 `cdb_*`，写进权限 0600 的文件，**不打印任何值**。

用法：
    python3 scripts/dump_cookies.py                 # 默认写到 ~/.cache/hipda/cookies.txt
    python3 scripts/dump_cookies.py --out /tmp/c.txt --jar <另一个 binarycookies>

注意：这是真实登录凭据。用完 `rm` 掉，不要提交、不要贴进聊天记录。
"""

import argparse
import os
import pathlib
import struct
import sys

WANTED = ("cdb_auth", "cdb_sid", "cdb_cookietime", "cdb_oldtopics", "cdb_visitedfid")


def parse_binarycookies(raw: bytes) -> list[tuple[str, str, str, str]]:
    """返回 [(domain, name, path, value)]。"""
    if raw[:4] != b"cook":
        raise ValueError("不是 binarycookies 文件")
    pages = struct.unpack(">I", raw[4:8])[0]
    sizes = [struct.unpack(">I", raw[8 + 4 * i : 12 + 4 * i])[0] for i in range(pages)]
    offset = 8 + 4 * pages
    out = []
    for size in sizes:
        page = raw[offset : offset + size]
        offset += size
        count = struct.unpack("<I", page[4:8])[0]
        for i in range(count):
            start = struct.unpack("<I", page[8 + 4 * i : 12 + 4 * i])[0]
            cookie = page[start:]

            def field(index: int) -> str:
                at = struct.unpack("<I", cookie[index : index + 4])[0]
                end = cookie.index(b"\x00", at)
                return cookie[at:end].decode("utf-8", "ignore")

            out.append((field(16), field(20), field(24), field(28)))
    return out


def main() -> int:
    default_jar = (
        pathlib.Path.home()
        / "Library/HTTPStorages/nsh-app-hipda-nsh-rust-macos.binarycookies"
    )
    ap = argparse.ArgumentParser(description="导出论坛 cookie")
    ap.add_argument("--jar", default=str(default_jar), help="binarycookies 路径")
    ap.add_argument(
        "--out",
        default=str(pathlib.Path.home() / ".cache/hipda/cookies.txt"),
        help="输出文件（会设为 0600）",
    )
    args = ap.parse_args()

    raw = pathlib.Path(args.jar).read_bytes()
    cookies = parse_binarycookies(raw)
    picked = [(name, value) for _, name, _, value in cookies if name in WANTED and value]
    if not any(n == "cdb_auth" for n, _ in picked):
        print("✗ cookie jar 里没有 cdb_auth：先在 App 里登录一次", file=sys.stderr)
        return 1

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    os.chmod(out.parent, 0o700)
    out.write_text("; ".join(f"{n}={v}" for n, v in picked))
    os.chmod(out, 0o600)

    print(f"✓ {len(picked)} 个 cookie → {out}（0600，内容不打印）")
    for name, value in picked:
        print(f"  {name:<18} len={len(value)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
