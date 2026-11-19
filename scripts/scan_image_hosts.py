#!/usr/bin/env python3
"""扫描 HTML 页面里的外链图片域名，核对 nsh.json 的 network.hosts 白名单。

用法：
    python3 scripts/scan_image_hosts.py                     # 扫描仓库内真实样本
    python3 scripts/scan_image_hosts.py /tmp/thread.html    # 追加扫描新抓的页面
    python3 scripts/scan_image_hosts.py --json              # 机器可读输出

默认只读，不修改任何文件。发现未声明域名时退出码为 1，并打印可直接粘贴的
hosts 条目；人工确认域名可信后，再编辑 nsh.json 并重跑 cargo test / nsh test。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "nsh.json"
SAMPLE_DIR = ROOT / "crates" / "forum-core" / "testdata"

# Discuz! 7.2 正文图片：缩略图在 src，原图在 file；CSS 背景图不在本次范围。
IMG_TAG = re.compile(r"<img\b[^>]*>", re.I | re.S)
ATTR = re.compile(r"""\b(src|file)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))""", re.I)
ABSOLUTE = re.compile(r"^(?:https?:)?//([^/?#\s]+)", re.I)


def decode(raw: bytes) -> str:
    """样本里 GBK 与 UTF-8 混杂；URL 都是 ASCII，任选一种可解码即可。"""
    for enc in ("utf-8", "gb18030"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", "replace")


def declared_hosts() -> list[str]:
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    hosts = data["capabilities"]["network"]["hosts"]
    if not isinstance(hosts, list) or not hosts:
        raise SystemExit(f"nsh.json 未声明 capabilities.network.hosts：{MANIFEST}")
    return [str(h).strip() for h in hosts if str(h).strip()]


def normalized(rule: str) -> str:
    return rule.lower().lstrip("*.")


def host_of(url: str) -> str | None:
    m = ABSOLUTE.match(url.strip())
    if not m:
        return None
    host = m.group(1).split(":")[0].lower()
    return host or None


def covered(host: str, rules: list[str]) -> bool:
    return any(host == r or host.endswith("." + r) for r in rules)


def image_urls(text: str):
    for tag in IMG_TAG.finditer(text):
        for m in ATTR.finditer(tag.group(0)):
            url = m.group(2) or m.group(3) or m.group(4) or ""
            if url:
                yield url


def scan(paths: list[Path], rules: list[str]) -> tuple[Counter, dict[str, str]]:
    counts: Counter = Counter()
    examples: dict[str, str] = {}
    for path in paths:
        for url in image_urls(decode(path.read_bytes())):
            host = host_of(url)
            if host and not covered(host, rules):
                counts[host] += 1
                examples.setdefault(host, f"{path.name}: {url[:120]}")
    return counts, examples


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="*", type=Path, help="额外要扫描的 HTML 文件")
    parser.add_argument("--json", action="store_true", help="以 JSON 输出，便于 CI 消费")
    args = parser.parse_args()

    declared = declared_hosts()
    rules = [normalized(h) for h in declared]
    paths = sorted(SAMPLE_DIR.glob("*.html")) + list(args.files)
    missing_paths = [str(p) for p in paths if not p.is_file()]
    if missing_paths:
        raise SystemExit(f"文件不存在：{', '.join(missing_paths)}")

    counts, examples = scan(paths, rules)
    if args.json:
        print(json.dumps({
            "declared_hosts": declared,
            "scanned_files": [str(p.relative_to(ROOT)) if p.is_relative_to(ROOT) else str(p) for p in paths],
            "unlisted_hosts": dict(counts),
            "examples": examples,
        }, ensure_ascii=False, indent=2))
        return 1 if counts else 0

    print(f"已声明域名：{', '.join(declared)}")
    print(f"扫描文件：{len(paths)} 个")
    if not counts:
        print("✓ 所有外链图片域名均在白名单内")
        return 0

    print(f"⚠ 发现 {len(counts)} 个未声明的外链图片域名：")
    for host, n in sorted(counts.items()):
        print(f"  - {host}（{n} 处）  例：{examples[host]}")
    print("\n确认域名可信后，把下列条目加入 nsh.json 的 capabilities.network.hosts：")
    print(json.dumps(sorted(counts), ensure_ascii=False))
    print("然后重跑：cargo test --workspace && ~/.local/bin/nsh test")
    return 1


if __name__ == "__main__":
    sys.exit(main())
