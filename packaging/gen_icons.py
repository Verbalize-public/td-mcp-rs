#!/usr/bin/env python3
"""Regenerate the app icon assets from the master `logo.svg`.

Supersedes the old Pillow-based `gen_ico.py` (ICO sizes are now rendered
straight from the SVG, so no image library is needed).

Outputs (all committed, so this only needs to run when the logo changes):

- crates/tdmcp-gui/assets/logo-mark.png      sidebar / tray-popup brand mark
- crates/tdmcp-gui/assets/icon-normal.png    tray + window icon (healthy)
- crates/tdmcp-gui/assets/icon-attention.png tray icon + orange attention badge
- packaging/windows/app.ico                  multi-size ICO for the installer

Requires the `resvg` CLI (https://github.com/linebender/resvg) on PATH —
`cargo binstall resvg` or `cargo install resvg`. `rsvg-convert` is used as a
fallback when present. Run from the repo root:

    python3 packaging/gen_icons.py
"""

import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "logo.svg"
ASSETS = ROOT / "crates" / "tdmcp-gui" / "assets"
ICO_DST = ROOT / "packaging" / "windows" / "app.ico"

# Node-square crop: the logo's main frame spans 112..400 on a 512 canvas;
# each variant pads it a little for the SVG glow filter bleed.
MARK_VIEWBOX = "88 88 336 336"  # brand mark — soft glow margin
ICON_VIEWBOX = "96 96 320 320"  # tray/window icon — artwork nearly fills
ATTN_VIEWBOX = "56 56 400 400"  # leaves room for the corner badge

# Attention badge: theme-ACCENT orange dot pinned to the frame's top-right
# corner, dark ring for separation (same convention as the previous PNG set).
ATTN_BADGE = (
    '<circle cx="400" cy="112" r="52" fill="#ff7a1a" '
    'stroke="#121620" stroke-width="14"/>'
)

ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def renderer() -> tuple[str, list[str]]:
    if shutil.which("resvg"):
        return "resvg", ["--skip-system-fonts"]
    if shutil.which("rsvg-convert"):
        return "rsvg-convert", []
    sys.exit("error: need `resvg` (cargo binstall resvg) or `rsvg-convert` on PATH")


def render(svg: Path, out: Path, size: int, tool: str, extra: list[str]) -> None:
    if tool == "resvg":
        subprocess.run(
            [tool, *extra, "--width", str(size), "--height", str(size), str(svg), str(out)],
            check=True,
        )
    else:
        subprocess.run(
            [tool, "-w", str(size), "-h", str(size), str(svg), "-o", str(out)],
            check=True,
        )


def swap_viewbox(svg_text: str, viewbox: str) -> str:
    old = svg_text.split('viewBox="', 1)[1].split('"', 1)[0]
    return svg_text.replace(f'viewBox="{old}"', f'viewBox="{viewbox}"', 1)


def write_ico(sizes_png: dict[int, bytes], dst: Path) -> None:
    """PNG-compressed ICO (Vista+): ICONDIR + entries + PNG blobs."""
    entries = b""
    blobs = b""
    offset = 6 + 16 * len(sizes_png)
    for size in sorted(sizes_png):
        png = sizes_png[size]
        b = size if size < 256 else 0
        entries += struct.pack("<BBBBHHII", b, b, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    dst.write_bytes(struct.pack("<HHH", 0, 1, len(sizes_png)) + entries + blobs)


def main() -> None:
    tool, extra = renderer()
    src_text = SRC.read_text()
    ASSETS.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        mark_svg = tmp / "mark.svg"
        icon_svg = tmp / "icon.svg"
        attn_svg = tmp / "attention.svg"
        mark_svg.write_text(swap_viewbox(src_text, MARK_VIEWBOX))
        icon_svg.write_text(swap_viewbox(src_text, ICON_VIEWBOX))
        attn_svg.write_text(swap_viewbox(src_text, ATTN_VIEWBOX).replace("</svg>", ATTN_BADGE + "</svg>", 1))

        render(mark_svg, ASSETS / "logo-mark.png", 256, tool, extra)
        render(icon_svg, ASSETS / "icon-normal.png", 512, tool, extra)
        render(attn_svg, ASSETS / "icon-attention.png", 512, tool, extra)

        ico_pngs: dict[int, bytes] = {}
        for size in ICO_SIZES:
            out = tmp / f"ico-{size}.png"
            render(icon_svg, out, size, tool, extra)
            ico_pngs[size] = out.read_bytes()
        write_ico(ico_pngs, ICO_DST)

    for name in ("logo-mark.png", "icon-normal.png", "icon-attention.png"):
        print(f"wrote {ASSETS / name}")
    print(f"wrote {ICO_DST}")


if __name__ == "__main__":
    main()
