#!/usr/bin/env python3
"""Render the canonical application SVG into the shipped PNG and Windows ICO.

Requires ImageMagick (`magick`). Run after editing build/icon.svg.
"""

from pathlib import Path
import shutil
import subprocess


def main():
    root = Path(__file__).resolve().parent.parent
    magick = shutil.which("magick")
    if not magick:
        raise SystemExit("Install ImageMagick to regenerate the application icons.")
    svg, png, ico = (root / "build" / f"icon.{ext}" for ext in ("svg", "png", "ico"))
    subprocess.run(
        [magick, "-background", "none", str(svg), "-resize", "256x256", "-strip", f"PNG32:{png}"],
        check=True,
    )
    subprocess.run(
        [magick, str(png), "-define", "icon:auto-resize=256,128,64,48,40,32,24,20,16", "-strip", str(ico)],
        check=True,
    )
    print("Generated build/icon.png and build/icon.ico from build/icon.svg")


if __name__ == "__main__":
    main()
