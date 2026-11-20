"""Smoke test of the *packaged* exe: unzip the release into a clean folder and drive it (sandbox data dir, real profiles read-only).
Run:  .venv\\Scripts\\python tests\\smoke_release.py [path\\to\\release.zip]"""
import ctypes, json, os, subprocess, sys, tempfile, time, zipfile
from ctypes import wintypes as wt
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
import core
profiles = core.detect_profiles()
zips = sorted((ROOT / "release").glob("*.zip"))
ZIP = Path(sys.argv[1]) if len(sys.argv) > 1 else zips[-1]
WORK = Path(tempfile.mkdtemp(prefix="cu_rel_")); SANDBOX = WORK / "appdata"; (SANDBOX / "claude-usage").mkdir(parents=True)
with zipfile.ZipFile(ZIP) as z: z.extractall(WORK / "unzipped")
EXE = next((WORK / "unzipped").rglob("claude-usage.exe"))
DATA = SANDBOX / "claude-usage"; env = {**os.environ, "LOCALAPPDATA": str(SANDBOX)}
results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok); print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

u = ctypes.windll.user32
u.FindWindowW.restype = wt.HWND
def visible_window(title):
    found = []
    CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(h, _):
        b = ctypes.create_unicode_buffer(64); u.GetWindowTextW(h, b, 64)
        if b.value == title and u.IsWindowVisible(h): found.append(h)
        return True
    u.EnumWindows(CB(cb), None); return bool(found)
def procs():
    out = subprocess.run(["powershell", "-NoProfile", "-Command", "(Get-CimInstance Win32_Process | ? { $_.ExecutablePath -like '" + str(WORK).replace("'", "''") + "*' } | Measure-Object).Count"], capture_output=True, text=True).stdout
    return int(out.strip() or 0)
def kill_all():
    subprocess.run(["powershell", "-NoProfile", "-Command", "Get-CimInstance Win32_Process | ? { $_.ExecutablePath -like '" + str(WORK).replace("'", "''") + "*' } | % { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"], capture_output=True)
def hook(cache, cfg_dir):
    d = SANDBOX / "hk" / "claude-usage"; d.mkdir(parents=True, exist_ok=True); f = d / "usage_cache.json"; f.write_text(json.dumps(cache)) if cache else f.unlink(missing_ok=True)
    return subprocess.run([str(EXE), "hook"], env={**os.environ, "LOCALAPPDATA": str(SANDBOX / "hk"), "CLAUDE_CONFIG_DIR": str(cfg_dir)}, capture_output=True, timeout=60)

print(f"package: {ZIP.name}  ({ZIP.stat().st_size/1e6:.1f} MB)\nunzipped to: {EXE.parent}")
check("zip contains exe + README + CHANGELOG", {p.name for p in EXE.parent.iterdir()} >= {"claude-usage.exe", "README.md", "CHANGELOG.md"}, ", ".join(p.name for p in EXE.parent.iterdir()))
vi = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Item '{EXE}').VersionInfo | Select ProductName,ProductVersion,FileDescription | ConvertTo-Json"], capture_output=True, text=True).stdout
vi = json.loads(vi); check("exe has product name / version / description", vi["ProductVersion"] == core.__version__ and vi["ProductName"] == "claude-usage", str(vi))

core.DATA = DATA
cfg = dict(core.DEFAULTS, profiles=profiles, widget_enabled=True, autostart=False, block_enabled=True, warn_pct=101, block_pct=102)   # thresholds unreachable: no toasts
(DATA / "config.json").write_text(json.dumps(cfg), "utf-8")
t0 = time.time(); p = subprocess.Popen([str(EXE), "--hidden"], env=env)
time.sleep(22)
check("app is running (hidden start)", p.poll() is None and procs() >= 1, f"{procs()} process(es)")
check("no dashboard window on hidden start", not visible_window("claude-usage"))
c = json.loads((DATA / "usage_cache.json").read_text("utf-8")) if (DATA / "usage_cache.json").exists() else {}
have = [n for n, v in (c.get("profiles") or {}).items() if "five" in v]
check("fetched usage for all profiles", len(have) == len(profiles), f"{len(have)}/{len(profiles)}; errors={ {n: v.get('err') for n, v in (c.get('profiles') or {}).items() if v.get('err')} }")
ex = {n: v.get("extra") for n, v in (c.get("profiles") or {}).items() if v.get("extra")}
check("credit (extra usage) + plan parsed from the real API for the hover card", len(ex) >= 1 and all("used" in e and "limit" in e for e in ex.values()) and any(v.get("plan") for v in (c.get("profiles") or {}).values()), f"{len(ex)} account(s) with credit data: " + ", ".join(f"{n}=${e['used']:.2f}/${e['limit']:.2f}" for n, e in ex.items()))
check("taskbar widget is on screen", visible_window("claude-usage-widget"))
t1 = time.time(); subprocess.Popen([str(EXE)], env=env)
while time.time() - t1 < 15 and not visible_window("claude-usage"): time.sleep(0.2)
check("second launch opens the dashboard (single instance)", visible_window("claude-usage"), f"{time.time()-t1:.1f}s")
n_before = procs(); subprocess.Popen([str(EXE)], env=env); time.sleep(3)
check("launching again does not spawn more processes", procs() == n_before, f"{n_before} -> {procs()}")
X = SANDBOX / "X"; X.mkdir()
hc = lambda f, w: {"ts": time.time(), "block_enabled": True, "block_pct": 95, "profiles": {"me": {"path": str(X), "five": f, "week": w}}}
r_ok, r_block, r_none = hook(hc(10, 10), X), hook(hc(97, 10), X), hook(None, X)
check("hook (packaged windowed exe): allow below threshold", r_ok.returncode == 0 and r_ok.stderr == b"")
check("hook: blocks above threshold with a readable message", r_block.returncode == 2 and "Đổi sang" in r_block.stderr.decode("utf-8", "replace"), r_block.stderr.decode("utf-8", "replace").strip()[:90])
check("hook: fails open with no cache", r_none.returncode == 0)
kill_all(); time.sleep(2)
check("everything shut down cleanly", procs() == 0, f"{procs()} left")
print(f"\n{sum(results)}/{len(results)} checks passed")
sys.exit(0 if all(results) else 1)
