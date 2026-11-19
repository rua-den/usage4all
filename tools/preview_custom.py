"""Preview of the appearance options (labels, font scale, plate, theme, hidden accounts) across styles."""
import sys
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import core, widget

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
cache = {"busy": [], "profiles": {
    "default (desktop)": {"five": 12, "week": 43, "five_reset": iso(3 * 3600), "week_reset": iso(30 * 3600)},
    "claude-da": {"five": 23, "week": 49, "five_reset": iso(3.2 * 3600), "week_reset": iso(20 * 3600)},
    "claude-hn": {"five": 90, "week": 37, "five_reset": iso(2.3 * 3600), "week_reset": iso(40 * 3600)},
    "claude-ngoai": {"five": 58, "week": 69, "five_reset": iso(1.1 * 3600), "week_reset": iso(80 * 3600)}}}
variants = [
    ("viết tắt (cũ)", {}),
    ("tên ngắn", {"widget_label": "short"}),
    ("tên tự đặt + chữ 130%", {"aliases": {"default (desktop)": "Chính", "claude-da": "Công ty", "claude-hn": "HN", "claude-ngoai": "Ngoài"}, "widget_font": 130}),
    ("nền mờ + tên ngắn", {"widget_label": "short", "widget_plate": True}),
]
cols = []
for style in widget.STYLES:
    for title, over in variants:
        cfg = dict(core.load_config(), widget_style=style, widget_reset=False, **over)
        cv, W, H, cw, ch = widget.render(cfg, cache, 2.0, -1, True, 96, 36, 0)
        img, _ = widget.compose(cv, W, H, 2.0, False, 0); cols.append((style, title, img))
cw_ = max(i.width for *_, i in cols) + 24; ch_ = max(i.height for *_, i in cols)
sheet = Image.new("RGBA", (cw_ * len(variants) + 20, (ch_ + 20) * len(widget.STYLES)), (26, 26, 28, 255))
for idx, (style, title, img) in enumerate(cols):
    r, c = divmod(idx, len(variants)); sheet.alpha_composite(img, (10 + c * cw_ + (cw_ - img.width) // 2, 10 + r * (ch_ + 20)))
sheet.save(sys.argv[1]); print(sheet.size, [v[0] for v in variants])
