#!/usr/bin/env python3
"""Regenerate the per-language CJK help-bubble font subsets.

For each of zh / ja / ko, subset the full Noto Sans CJK regional OTF
(assets/fonts/NotoSansCJK{sc,jp,kr}-Regular.otf) down to just the glyphs that
locale actually uses (assets/locales/{zh,ja,ko}.json) plus the three language
names (中文 / 日本語 / 한국어, so the language picker always renders), writing
assets/fonts/NotoSansCJK{sc,jp,kr}-subset.otf — the files embedded by
src/ui/fonts.rs.

Run after editing the zh/ja/ko translations:

    pip install fonttools
    python scripts/subset_cjk.py
"""
import json
import os

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")


def load(code):
    with open(os.path.join(ASSETS, "locales", code + ".json"), encoding="utf-8") as f:
        return json.load(f)


def chars_of(code):
    s = set()
    for v in load(code).values():
        s.update(v)
    return s


# Language-name glyphs must render under whichever CJK font is active (the
# picker lists every language at once), so each subset includes all three names.
names = set(load("zh")["_name"]) | set(load("ja")["_name"]) | set(load("ko")["_name"])

JOBS = [
    ("zh", "NotoSansCJKsc-Regular.otf", "NotoSansCJKsc-subset.otf"),
    ("ja", "NotoSansCJKjp-Regular.otf", "NotoSansCJKjp-subset.otf"),
    ("ko", "NotoSansCJKkr-Regular.otf", "NotoSansCJKkr-subset.otf"),
]

for code, src, out in JOBS:
    want = chars_of(code) | names
    opts = subset.Options(desubroutinize=True, hinting=False)
    opts.layout_features = []
    font = TTFont(os.path.join(ASSETS, "fonts", src), fontNumber=0)
    ss = subset.Subsetter(options=opts)
    ss.populate(unicodes=[ord(c) for c in want])
    ss.subset(font)
    outp = os.path.join(ASSETS, "fonts", out)
    font.save(outp)

    check = TTFont(outp, lazy=True)
    cmap = set().union(*[t.cmap.keys() for t in check["cmap"].tables])
    missing = sum(1 for c in want if ord(c) not in cmap)
    print(f"{code} -> {out}: {len(want)} glyphs, missing={missing}, {os.path.getsize(outp)} bytes")
    assert missing == 0, f"{code}: {missing} glyphs missing from {src}"
