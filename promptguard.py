"""Claude Code UserPromptSubmit hook: block prompt if this profile is over the block threshold.
Reads the cache written by the daemon. Fails open (allows) if cache is missing/stale/unknown."""
import json, os, sys, time
from pathlib import Path


def _err(msg):
    # windowed exe has no sys.stderr: write straight to the inherited stderr handle
    data = msg.encode("utf-8")
    try:
        import ctypes
        k = ctypes.windll.kernel32
        k.GetStdHandle.restype = ctypes.c_void_p
        h = k.GetStdHandle(-12)
        n = ctypes.c_ulong()
        if h and h != ctypes.c_void_p(-1).value:
            k.WriteFile(ctypes.c_void_p(h), data, len(data), ctypes.byref(n), None)
            return
    except Exception:
        pass
    sys.stderr.write(msg)


def run():
    cache = Path(os.environ.get("LOCALAPPDATA", Path.home())) / "claude-usage" / "usage_cache.json"
    try:
        c = json.loads(cache.read_text("utf-8"))
        if not c.get("block_enabled") or time.time() - c["ts"] > 900:
            return 0
        me = os.path.normcase(os.path.abspath(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude"))
        profs = c["profiles"]
        cur = next((n for n, p in profs.items() if os.path.normcase(os.path.abspath(p["path"])) == me), None)
        if not cur or "five" not in profs[cur]:
            return 0
        lim, p = c["block_pct"], profs[cur]
        if max(p["five"], p["week"]) < lim:
            return 0
        ok = [(n, max(q["five"], q["week"])) for n, q in profs.items() if "five" in q and n != cur]
        best = min(ok, key=lambda x: x[1])[0] if ok else "?"
        _err(f"Profile '{cur}' đã {p['five']:.0f}% (5h) / {p['week']:.0f}% (7d) >= {lim}%. "
              f"Đổi sang profile '{best}' hoặc tắt chặn trong claude-usage.\n")
        return 2
    except Exception:
        return 0

