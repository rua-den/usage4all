"""Render every widget style on dark and light taskbar colours into one PNG (for design review)."""
import sys, time
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import core, widget

out = Path(sys.argv[1])
iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
cache = {"busy": ["claude-hn"], "profiles": {
    "default (desktop)": {"five": 0, "week": 43, "five_reset": None, "week_reset": iso(30 * 3600)},
    "claude-da": {"five": 23, "week": 49, "five_reset": iso(3.2 * 3600), "week_reset": iso(20 * 3600)},
    "claude-hn": {"five": 90, "week": 37, "five_reset": iso(2.3 * 3600), "week_reset": iso(40 * 3600)},
    "claude-ngoai": {"five": 58, "week": 69, "five_reset": iso(1.1 * 3600), "week_reset": iso(80 * 3600), "err": "HTTP 429"}}}
cfg = core.load_config(); cfg["widget_reset"] = True
cols = []
for style in widget.STYLES:
    cfg["widget_style"] = style
    for vertical in (True, False):
        cv, W, H, cw, ch = widget.render(cfg, cache, 2.0, -1, vertical, 96, 36, 3)
        img, _ = widget.compose(cv, W, H, 2.0, False, 0)
        cols.append((style, vertical, img))
rowsV = [c for c in cols if c[1]]; rowsH = [c for c in cols if not c[1]]
Wt = sum(i.width for _, _, i in rowsV) + 40 * (len(rowsV) + 1)
Ht = max(i.height for _, _, i in rowsV) + sum(i.height + 14 for _, _, i in rowsH) + 100
sheet = Image.new("RGBA", (max(Wt, max(i.width for *_, i in rowsH) + 80), Ht), (24, 24, 26, 255))
x = 40
for _, _, i in rowsV: sheet.alpha_composite(i, (x, 30)); x += i.width + 40
y = 60 + max(i.height for _, _, i in rowsV)
for _, _, i in rowsH: sheet.alpha_composite(i, (40, y)); y += i.height + 14
sheet.save(out); print(sheet.size)
