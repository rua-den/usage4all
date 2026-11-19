"""Preview: 14 accounts on a 1200px-high screen, docked in a vertical taskbar, for every style."""
import sys
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import core, widget

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
names = ["default (desktop)", "claude-da", "claude-hn", "claude-ngoai"] + [f"claude-team-{i:02d}" for i in range(1, 11)]
cache = {"busy": [], "profiles": {n: {"five": (i * 17) % 100, "week": (i * 23) % 100, "five_reset": iso(3600 * (i % 5 + 1)), "week_reset": iso(86400 * (i % 6 + 1))} for i, n in enumerate(names)}}
outs = []
for style in widget.STYLES:
    cfg = dict(core.load_config(), widget_style=style, widget_sort="usage")
    items = widget.visible_profiles(cfg, cache)
    limit = widget.stack_limit(cfg, True, True, (1920, 1200))
    size, shown = widget.plan_layout(cfg, items, 1.0, True, 48, limit)
    view = {"profiles": widget.apply_plan(items, shown)}
    cv, W, H, cw, ch = widget.render(cfg, view, 1.0, -1, True, 48, size)
    img, _ = widget.compose(cv, W, H, 1.0, False, 0); outs.append(img)
    print(style, "size", round(size, 1), "shown", shown, "of", len(items), "height", H, "limit", limit)
sheet = Image.new("RGBA", (sum(i.width for i in outs) + 30 * (len(outs) + 1), max(i.height for i in outs) + 40), (26, 26, 28, 255)); x = 30
for i in outs: sheet.alpha_composite(i, (x, 20)); x += i.width + 30
sheet.save(sys.argv[1])
