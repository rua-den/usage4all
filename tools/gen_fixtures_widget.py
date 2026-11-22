"""Generate the Rust parity fixture for the widget's pure layout logic by running the real Python `widget` functions.

    .venv\\Scripts\\python tools\\gen_fixtures_widget.py

Writes rust/crates/core/tests/fixtures/widget.json (compared by rust/crates/core/tests/parity_widget.rs).
Sandbox rules as in the other generators: LOCALAPPDATA / USERPROFILE point at temp dirs before `core` is imported.
Nothing is drawn on screen; `widget.render` is only called to read the cell size it returns.
"""
import json
import os
import sys
import tempfile
from pathlib import Path

_TMP = tempfile.mkdtemp(prefix="cu-fixtures-widget-")
os.environ["LOCALAPPDATA"] = os.path.join(_TMP, "localappdata")
HOME = os.path.join(_TMP, "home")
os.makedirs(HOME)
os.environ["USERPROFILE"] = HOME
os.environ.pop("HOMEDRIVE", None)
os.environ.pop("HOMEPATH", None)
os.environ.pop("CLAUDE_CONFIG_DIR", None)

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
import core  # noqa: E402
import widget  # noqa: E402

OUT = ROOT / "rust" / "crates" / "core" / "tests" / "fixtures" / "widget.json"

NAMES = ["default", "claude-team-01", "claude-team-02", "claude-ngoai", "claude-super-long-account-name",
         "work", "claude-a", "claude-b", "claude-c", "claude-d", "claude-e", "claude-f", "claude-g", "claude-h"]


def prof(i, kind):
    if kind == "err":
        return {"err": "http 500", "status": 500}
    if kind == "wait":
        return {"err": "429", "status": 429, "five": 10.0, "week": 20.0}
    return {"five": float((i * 17) % 100), "week": float((i * 29) % 100),
            "five_reset": "2099-01-01T00:00:00+00:00", "week_reset": "2099-01-05T00:00:00+00:00"}


def cache(n, kinds=()):
    profs = {}
    for i, name in enumerate(NAMES[:n]):
        profs[name] = prof(i, kinds[i] if i < len(kinds) else "ok")
    return {"ts": 1.0, "profiles": profs, "busy": []}


CFGS = [
    {}, {"widget_style": "bars"}, {"widget_style": "cards"}, {"widget_style": "segments"}, {"widget_style": "arc"},
    {"widget_reset": False}, {"widget_show_5h": False}, {"widget_show_7d": False},
    {"widget_show_5h": False, "widget_show_7d": False}, {"widget_show_label": False, "widget_style": "arc"},
    {"widget_show_label": False, "widget_show_5h": False, "widget_show_7d": False},
    {"widget_show_reset7": False, "widget_style": "cards"}, {"widget_label": "short"},
    {"widget_sort": "usage"}, {"widget_hidden": ["work", "default"]}, {"widget_max_cells": 3},
    {"widget_max_len": 300, "widget_style": "bars"}, {"widget_size": 50, "widget_style": "segments"},
    {"widget_size": 24, "widget_max_cells": 5, "widget_style": "arc"}, {"aliases": {"work": "Wk", "default": ""}},
    {"widget_style": "bogus"}, {"widget_font": 140, "widget_opacity": 40, "widget_plate": True},
]


def jsonable(x):
    if isinstance(x, tuple):
        return [jsonable(i) for i in x]
    return x


def main():
    out = {"layout": [], "misc": {}}
    base = core.load_config()
    for ci, over in enumerate(CFGS):
        cfg = dict(base)
        cfg.update(over)
        sh, ms, rs = widget.shows(cfg), widget.metrics(cfg), widget.reset_lines(cfg)
        for n, kinds in ((0, ()), (1, ()), (4, ("ok", "err", "wait", "ok")), (7, ()), (14, ("ok", "err"))):
            c = cache(n, kinds)
            items = widget.visible_profiles(cfg, c)
            for scale in (1.0, 1.5):
                for vertical in (True, False):
                    for thick in (0, 48):
                        if not vertical and thick:
                            continue
                        for docked in (True, False):
                            limit = widget.stack_limit(cfg, vertical, docked, (1920, 1080))
                            size, shown = widget.plan_layout(cfg, items, scale, vertical, thick, limit)
                            view = widget.apply_plan(items, shown)
                            vc = dict(c)
                            vc["profiles"] = view
                            _, W, H, cw, ch = widget.render(cfg, vc, scale, -1, vertical, thick, size)
                            out["layout"].append({
                                "cfg": over, "n": n, "kinds": list(kinds), "scale": scale, "vertical": vertical,
                                "thick": thick, "docked": docked,
                                "shows": sh, "metrics": ms, "reset_lines": rs,
                                "names": [x[0] for x in items], "limit": limit,
                                "size": size, "shown": shown, "view": list(view.keys()),
                                "dims": [W, H, cw, ch]})
    m = out["misc"]
    m["pct_color"] = [[p, list(widget.pct_color(p, base))] for p in (0, 50, 79.9, 80, 94.9, 95, 100)]
    fills = []
    for plate in (False, True):
        for light in (False, True):
            for hov in (0, 1):
                cfg = dict(base, widget_plate=plate)
                fills.append([plate, light, hov, list(widget.cell_fill(cfg, light, hov))])
    m["cell_fill"] = fills
    m["tip"] = [[list(w), my, list(sz), list(sc), list(widget.tip_position(w, my, sz, sc))]
                for w, my, sz, sc in (((100, 50, 60, 300), 80, (250, 120), (1920, 1080)),
                                      ((1800, 50, 60, 300), 80, (250, 120), (1920, 1080)),
                                      ((1000, 1000, 60, 80), 1070, (250, 400), (1920, 1080)),
                                      ((0, 0, 10, 10), -50, (250, 120), (1920, 1080)))]
    m["font_opacity"] = [[v, widget.font_scale({"widget_font": v}), widget.opacity_alpha({"widget_opacity": v})]
                         for v in (0, 79, 100, 149, 200, 33.7, "x", None)]
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(out, f, ensure_ascii=False, indent=None, allow_nan=False)
        f.write("\n")
    print(f"widget: {len(out['layout'])} layout cases")


if __name__ == "__main__":
    main()
