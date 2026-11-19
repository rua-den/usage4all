"""Preview of the 'what to show' checkboxes across styles."""
import sys
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import core, widget

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
cache = {"busy": [], "profiles": {
    "default (desktop)": {"five": 12, "week": 43, "five_reset": iso(3 * 3600), "week_reset": iso(30 * 3600)},
    "claude-hn": {"five": 90, "week": 37, "five_reset": iso(2.3 * 3600), "week_reset": iso(40 * 3600)},
    "claude-ngoai": {"five": 58, "week": 69, "five_reset": iso(1.1 * 3600), "week_reset": iso(80 * 3600)}}}
variants = [("all", {}), ("5h only", {"widget_show_7d": False, "widget_show_reset7": False}), ("7d only", {"widget_show_5h": False, "widget_show_reset5": False}),
            ("no label", {"widget_show_label": False}), ("no resets", {"widget_reset": False}), ("minimal: 5h %", {"widget_show_label": False, "widget_show_7d": False, "widget_reset": False})]
cols = []
for style in widget.STYLES:
    for title, over in variants:
        cfg = dict(core.load_config(), widget_style=style, **over)
        cv, W, H, cw, ch = widget.render(cfg, cache, 2.0, -1, True, 96, 36, 0)
        img, _ = widget.compose(cv, W, H, 2.0, False, 0); cols.append(img)
cw_ = max(i.width for i in cols) + 20; ch_ = max(i.height for i in cols) + 16
sheet = Image.new("RGBA", (cw_ * len(variants) + 10, ch_ * len(widget.STYLES)), (26, 26, 28, 255))
for idx, img in enumerate(cols):
    r, c = divmod(idx, len(variants)); sheet.alpha_composite(img, (10 + c * cw_ + (cw_ - img.width) // 2, 8 + r * ch_))
sheet.save(sys.argv[1]); print(sheet.size)
