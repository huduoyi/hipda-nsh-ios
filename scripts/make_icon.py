#!/usr/bin/env python3
"""从一张方形插画生成 Windows 程序图标 `assets/icon.ico`（+ 同尺寸母版 png）。

用法：
    python scripts/make_icon.py <源图路径>

**为什么要分两套尺寸**：插画细节多，整体缩小到 16/24/32 px 时五官会糊成一团
（只剩一团黑发和白脸）。所以小尺寸改用「头部特写」重新裁切缩放，让脸在任务栏
和标题栏里还能认出来；48 px 以上仍用整图，保留校服和小熊。这是图标设计里
常见的 optical sizing，不是偷懒。

**为什么最高只到 256**：ICO 规范上限就是 256×256，Windows 从 Vista 起允许
PNG 压缩的 256 帧。

**为什么手写 ICO 目录**：Pillow 的 `save(format="ICO", sizes=..., append_images=...)`
在「基础图 + 追加图」混用时只写出了 3 帧（实测丢帧），而每帧都要用不同源图，
所以直接按 ICO 规范拼 header + 目录 + PNG 帧，逻辑更短也更好验证。
"""

from __future__ import annotations

import io
import struct
import sys
from pathlib import Path

from PIL import Image

FULL_SIZES = [(48, 48), (64, 64), (128, 128), (256, 256)]
SMALL_SIZES = [(16, 16), (24, 24), (32, 32)]

# 头部特写的裁切框（按 1080×1080 源图标定）：以脸为中心取正方形，
# 保留两只小熊发夹和一点校服领口
HEAD_BOX = (150, 40, 930, 820)


def build_ico(frames: list[tuple[tuple[int, int], Image.Image]]) -> bytes:
    """按 ICO 规范拼装：6 字节头 + 每帧 16 字节目录 + PNG 数据。"""
    payload = []
    for _size, im in frames:
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        payload.append(buf.getvalue())

    header = struct.pack("<HHH", 0, 1, len(frames))
    directory = b""
    offset = 6 + 16 * len(frames)
    for (w, h), data in zip((size for size, _ in frames), payload):
        directory += struct.pack(
            "<BBBBHHII",
            w if w < 256 else 0,
            h if h < 256 else 0,
            0,  # 调色板颜色数：真彩固定 0
            0,  # 保留位
            1,  # 色彩平面
            32,  # 位深
            len(data),
            offset,
        )
        offset += len(data)
    return header + directory + b"".join(payload)



def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    src_path = Path(sys.argv[1])
    out_dir = Path(__file__).resolve().parent.parent / "assets"
    out_dir.mkdir(exist_ok=True)

    src = Image.open(src_path).convert("RGB")
    if src.width != src.height:
        side = min(src.size)
        left = (src.width - side) // 2
        top = (src.height - side) // 2
        src = src.crop((left, top, left + side, top + side))

    full = src.resize((256, 256), Image.LANCZOS)
    head = src.crop(HEAD_BOX).resize((64, 64), Image.LANCZOS)

    frames = [(size, full.resize(size, Image.LANCZOS)) for size in FULL_SIZES]
    frames += [(size, head.resize(size, Image.LANCZOS)) for size in SMALL_SIZES]

    full.save(out_dir / "icon.png")
    (out_dir / "icon.ico").write_bytes(build_ico(frames))
    print(f"written {out_dir / 'icon.ico'} sizes={[size for size, _ in frames]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
