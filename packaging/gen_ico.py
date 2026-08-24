#!/usr/bin/env python3
"""Regenerate packaging/windows/app.ico from the tray icon source PNG.

Run after changing crates/tdmcp-gui/assets/icon-normal.png:
    pip install pillow && python packaging/gen_ico.py
"""

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "crates" / "tdmcp-gui" / "assets" / "icon-normal.png"
DST = ROOT / "packaging" / "windows" / "app.ico"

# Windows shell sizes; 256 is required for modern Explorer/views.
SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]


def main() -> None:
    img = Image.open(SRC)
    DST.parent.mkdir(parents=True, exist_ok=True)
    img.save(DST, format="ICO", sizes=SIZES)
    print(f"wrote {DST}")


if __name__ == "__main__":
    main()
