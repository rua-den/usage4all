"""Phase 0 baseline: time the widget's pure render path (no window) and the per-tick IO.
Run: .venv/Scripts/python bench/bench_widget.py  (uses a temporary LOCALAPPDATA, fake numbers only)"""
import os, sys, tempfile, time, statistics
from datetime import datetime, timedelta, timezone
from pathlib import Path

os.environ["LOCALAPPDATA"] = tempfile.mkdtemp(prefix="cu-bench-")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import core, widget  # noqa: E402

iso = lambda s: (datetime.now(timezone.utc) + timedelta(seconds=s)).isoformat()


def make(n):
    profs = [{"name": f"claude-p{i}", "path": f"C:/fake/p{i}"} for i in range(n)]
    data = {p["name"]: {"five": 10 + 6 * i % 90, "week": 20 + 5 * i % 80, "five_reset": iso(3600 * (i + 1)),
                        "week_reset": iso(86400 * (i + 1)), "fetched_at": time.time()} for i, p in enumerate(profs)}
    return profs, data


def ms(fn, reps):
    t = []
    for _ in range(reps):
        s = time.perf_counter(); fn(); t.append((time.perf_counter() - s) * 1000)
    return statistics.median(t), max(t)


print("style     n   render_med  render_max  plan_layout  load_cache  light_taskbar")
for n in (4, 14):
    profs, data = make(n)
    for style in ("rings", "bars", "cards", "segments", "arc"):
        cfg = dict(core.DEFAULTS, profiles=profs, widget_style=style)
        core.save_config(cfg); core.write_cache(cfg, data)
        cache = core.load_cache()
        items = widget.visible_profiles(cfg, cache)
        r_med, r_max = ms(lambda: widget.compose(*widget.render(cfg, cache, 1.0, -1, True, 48, 36.0, 3)[:3], 1.0, False, 0.0), 10)
        p_med, _ = ms(lambda: widget.plan_layout(cfg, items, 1.0, True, 48, 1000), 5)
        l_med, _ = ms(core.load_cache, 20)
        t_med, _ = ms(widget.light_taskbar, 20)
        print(f"{style:9} {n:2}   {r_med:8.1f}ms  {r_max:8.1f}ms  {p_med:9.1f}ms  {l_med:8.2f}ms  {t_med:8.2f}ms")
