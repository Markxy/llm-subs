"""Rebuilds bootstrap-icons-subset.ttf with exactly the icons listed in src/ui/icons.rs.

Bootstrap Icons only ships web fonts, which egui cannot read, and the whole set would add
300 KB to the app for the few dozen icons it uses.

    pip install fonttools
    python assets/subset_icons.py path/to/bootstrap-icons.woff

The .woff is in the bootstrap-icons npm package under font/fonts/; the current font was
made from version 1.13.1.
"""

import re
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ASSETS = Path(__file__).resolve().parent
LIST = ASSETS.parent / "src" / "ui" / "icons.rs"
OUTPUT = ASSETS / "bootstrap-icons-subset.ttf"


def main() -> None:
    code_points = sorted({int(code, 16) for code in re.findall(r'"\\u\{([0-9a-f]+)\}"', LIST.read_text(encoding="utf-8"))})
    font = TTFont(sys.argv[1])
    missing = [hex(code) for code in code_points if code not in font.getBestCmap()]
    if missing:
        sys.exit(f"not in this font: {', '.join(missing)}")

    options = subset.Options()
    options.layout_features = []
    options.notdef_outline = True
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=code_points)
    subsetter.subset(font)
    font.flavor = None
    font.save(OUTPUT)
    print(f"{len(code_points)} icons, {OUTPUT.stat().st_size} bytes: {OUTPUT}")


if __name__ == "__main__":
    main()
