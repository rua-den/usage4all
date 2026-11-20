"""Install / remove the Claude Code UserPromptSubmit hook in each profile's settings.json.
Only touches our own entry (marked by the command containing 'claude-usage' + ' hook'), keeps a one-time backup."""
import json, shutil, sys
from pathlib import Path
import core

MARK = "claude-usage"


def command():
    """The shell command Claude Code should run."""
    if getattr(sys, "frozen", False):
        return f'"{sys.executable}" hook'
    return f'"{sys.executable}" "{Path(core.__file__).with_name("main.py")}" hook'


def _is_ours(h):
    c = h.get("command", "") if isinstance(h, dict) else ""
    return MARK in c.lower() and c.rstrip().endswith(" hook")


def _load(p):
    try: return json.loads(p.read_text("utf-8")) if p.exists() else {}
    except Exception: return None          # unreadable/invalid JSON: never overwrite it


def _save(p, data, backup=True):
    if backup and p.exists():
        bak = p.with_name(p.name + ".claude-usage.bak")
        if not bak.exists(): shutil.copy2(p, bak)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, indent=2, ensure_ascii=False), "utf-8")


def _strip(data):
    """Remove our hook entries; returns how many were removed. Cleans up empty containers we may have created."""
    n = 0
    groups = (data.get("hooks") or {}).get("UserPromptSubmit") or []
    for g in list(groups):
        hs = g.get("hooks", [])
        keep = [h for h in hs if not _is_ours(h)]
        n += len(hs) - len(keep)
        if keep: g["hooks"] = keep
        else: groups.remove(g)
    if "hooks" in data:
        if not groups: data["hooks"].pop("UserPromptSubmit", None)
        if not data["hooks"]: data.pop("hooks")
    return n


def status(profiles):
    """[(name, installed: bool|None)]  None = settings.json unreadable."""
    out = []
    for p in profiles:
        data = _load(Path(p["path"]) / "settings.json")
        if data is None: out.append((p["name"], None)); continue
        gs = (data.get("hooks") or {}).get("UserPromptSubmit") or []
        out.append((p["name"], any(_is_ours(h) for g in gs for h in g.get("hooks", []))))
    return out


def install(profiles):
    res = []
    for p in profiles:
        f = Path(p["path"]) / "settings.json"
        data = _load(f)
        if data is None: res.append((p["name"], "bỏ qua: settings.json không đọc được")); continue
        _strip(data)
        data.setdefault("hooks", {}).setdefault("UserPromptSubmit", []).append(
            {"hooks": [{"type": "command", "command": command()}]})
        _save(f, data); res.append((p["name"], "đã cài"))
    return res


def uninstall(profiles):
    res = []
    for p in profiles:
        f = Path(p["path"]) / "settings.json"
        data = _load(f)
        if data is None: res.append((p["name"], "bỏ qua: settings.json không đọc được")); continue
        n = _strip(data)
        if n: _save(f, data, backup=False)
        res.append((p["name"], "đã gỡ" if n else "không có hook"))
    return res


def repair(profiles):
    """If the hook is installed but points at a different exe path (exe moved), rewrite it."""
    for p in profiles:
        f = Path(p["path"]) / "settings.json"
        data = _load(f)
        if not data: continue
        gs = (data.get("hooks") or {}).get("UserPromptSubmit") or []
        ours = [h for g in gs for h in g.get("hooks", []) if _is_ours(h)]
        if ours and any(h.get("command") != command() for h in ours):
            _strip(data)
            data.setdefault("hooks", {}).setdefault("UserPromptSubmit", []).append(
                {"hooks": [{"type": "command", "command": command()}]})
            _save(f, data, backup=False)
