"""Preview of the hover detail card."""
import sys, time
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import core, widget

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()
now = time.time()
full = {"five": 90, "week": 37, "five_reset": iso(2.3 * 3600), "week_reset": iso(40 * 3600), "fetched_at": now - 12, "plan": "team",
        "extra": {"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"}, "scoped": [{"name": "Fable", "pct": 21, "resets_at": iso(40 * 3600)}]}
stale = dict(full, five=58, week=69, err="HTTP 429", retry_at=now + 200, fetched_at=now - 400, extra=None, scoped=[])
bad = {"err": "token hết hạn - chạy claude ở profile này 1 lần", "status": 401}
cfg = core.load_config()
imgs = [widget.render_tip("claude-hn", full, cfg, 1.5), widget.render_tip("claude-ngoai", stale, cfg, 1.5), widget.render_tip("default (desktop)", bad, cfg, 1.5),
        widget.render_tip("claude-hn", full, dict(cfg, widget_theme="light"), 1.5)]
W = sum(i.width for i in imgs) + 30 * (len(imgs) + 1); H = max(i.height for i in imgs) + 60
sheet = Image.new("RGBA", (W, H), (60, 60, 66, 255)); x = 30
for i in imgs: sheet.alpha_composite(i, (x, 30)); x += i.width + 30
sheet.save(sys.argv[1]); print("ok", [i.size for i in imgs])
