"""Smoke test of the PACKAGED Rust build: unzip the newest release\\claude-usage-rust-v*-win-x64.zip into a clean
folder and drive the real exes. Everything is sandboxed (LOCALAPPDATA / USERPROFILE point to temp dirs, a fake home
with ONE profile that has NO credentials file, so the daemon never reads a token and never sends a request;
CU_NO_AUTOSTART / CU_NO_REPAIR keep it away from HKCU and the real settings.json). No real token, no network.

Run:  .venv\\Scripts\\python tests\\smoke_release_rust.py [path\\to\\zip]
One GUI test at a time. Refuses to run if a Rust tray / dashboard window already exists. Only processes whose exe
lives inside this test's temp folder are ever stopped (the user's own Python claude-usage.exe is never touched)."""
import ctypes, hashlib, json, os, shutil, subprocess, sys, tempfile, time, zipfile
from ctypes import wintypes as wt
from pathlib import Path

sys.stdout.reconfigure(encoding='utf-8', errors='replace')
ROOT = Path(__file__).resolve().parent.parent
zips = sorted((ROOT / "release").glob("claude-usage-rust-v*-win-x64.zip"), key=lambda p: p.stat().st_mtime)
ZIP = Path(sys.argv[1]) if len(sys.argv) > 1 else (zips[-1] if zips else None)
if ZIP is None or not ZIP.exists():
    sys.exit("no release\\claude-usage-rust-v*-win-x64.zip: run build-rust.ps1 first")

results = []
def check(name, ok, detail=""):
    ok = bool(ok); results.append(ok)
    print(f"  {'ok  ' if ok else 'FAIL'} {name}" + (f"  [{detail}]" if detail else ""), flush=True)

# ---- win32 helpers (private argtypes on a private WinDLL) ----
u = ctypes.WinDLL("user32", use_last_error=True)
u.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM), wt.LPARAM]
u.GetClassNameW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
u.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
u.IsWindowVisible.argtypes = [wt.HWND]
u.PostMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
WM_CLOSE, WM_COMMAND, ID_QUIT = 0x10, 0x111, 6

def windows(cls):
    """[(hwnd, pid, visible)] of every top-level window of this class (hidden ones included)."""
    out = []
    def cb(h, _):
        b = ctypes.create_unicode_buffer(64); u.GetClassNameW(h, b, 64)
        if b.value == cls:
            pid = wt.DWORD(); u.GetWindowThreadProcessId(h, ctypes.byref(pid)); out.append((h, pid.value, bool(u.IsWindowVisible(h))))
        return True
    u.EnumWindows(ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)(cb), 0)
    return out

def wait_for(fn, secs, step=0.1):
    t = time.time()
    while time.time() - t < secs:
        v = fn()
        if v: return v
        time.sleep(step)
    return fn()

def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""): h.update(b)
    return h.hexdigest()

def read_json(p):
    try: return json.loads(Path(p).read_text("utf-8"))
    except Exception: return None

# ---- refuse to run next to another Rust instance ----
busy = windows("ClaudeUsageTray") + windows("ClaudeUsageDashboard")
if busy:
    sys.exit("another Rust claude-usage tray/dashboard window exists: quit it first (GUI tests must not overlap)")

WORK = Path(tempfile.mkdtemp(prefix="cu_rust_rel_"))
UNZ = WORK / "unzipped"
SANDBOX = WORK / "appdata"; DATA = SANDBOX / "claude-usage"; DATA.mkdir(parents=True)
HOME = WORK / "home"; PROF = HOME / "claude-profiles" / "claude-test"; PROF.mkdir(parents=True)   # NO .credentials.json
procs_started = []

def procs():
    """[(pid, name)] of processes whose exe lives inside WORK."""
    ps = ("Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith('"
          + str(WORK).replace("'", "''") + "', [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { \"$($_.ProcessId) $($_.Name)\" }")
    out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True).stdout
    return [(int(l.split()[0]), l.split()[1]) for l in out.splitlines() if l.strip()]

def kill_ours():
    for pid, _ in procs():
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)

def env(extra=None):
    e = {k: v for k, v in os.environ.items() if k not in ("HOMEDRIVE", "HOMEPATH", "CLAUDE_CONFIG_DIR", "CU_NO_DASHBOARD")}
    e.update(LOCALAPPDATA=str(SANDBOX), USERPROFILE=str(HOME), CU_NO_AUTOSTART="1", CU_NO_REPAIR="1")
    e.update(extra or {})
    return e

try:
    print(f"package: {ZIP.name}  ({ZIP.stat().st_size/1e6:.2f} MB, built {time.strftime('%H:%M:%S', time.localtime(ZIP.stat().st_mtime))})")
    # ---- package integrity ----
    sidecar = ZIP.with_name(ZIP.name + ".sha256")
    want = sidecar.read_text().split()[0] if sidecar.exists() else None
    check("zip sha256 matches its .sha256 file", want and sha256(ZIP) == want, (want or "no .sha256")[:16])
    with zipfile.ZipFile(ZIP) as z: z.extractall(UNZ)
    names = {p.name for p in UNZ.iterdir()}
    check("zip holds both exes + SHA256SUMS + README + CHANGELOG", names >= {"claude-usage.exe", "claude-usage-dashboard.exe", "SHA256SUMS.txt", "README.md", "CHANGELOG.md"}, ", ".join(sorted(names)))
    sums = {l.split()[1].lstrip("*"): l.split()[0] for l in (UNZ / "SHA256SUMS.txt").read_text().splitlines() if l.strip()}
    check("SHA256SUMS.txt matches the unpacked exes", sums and all(sha256(UNZ / n) == h for n, h in sums.items()) and set(sums) == {"claude-usage.exe", "claude-usage-dashboard.exe"}, str(list(sums)))
    EXE, DASH = UNZ / "claude-usage.exe", UNZ / "claude-usage-dashboard.exe"
    vi = json.loads(subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Item '{EXE}').VersionInfo | Select ProductName,ProductVersion,FileDescription | ConvertTo-Json"], capture_output=True, text=True).stdout)
    ver = ZIP.name.split("-v", 1)[1].rsplit("-win", 1)[0]
    check("exe has product name / version (= zip name) / description", vi["ProductName"] == "claude-usage" and vi["ProductVersion"] == ver and vi["FileDescription"], str(vi))
    check("exe sizes sane (tray < 6 MB, dashboard < 3 MB)", EXE.stat().st_size < 6e6 and DASH.stat().st_size < 3e6, f"{EXE.stat().st_size/1e6:.2f} / {DASH.stat().st_size/1e6:.2f} MB")
    # test-only seams (endpoint override, widget state dump, fake taskbar/fullscreen) must be compiled OUT of the shipped exe
    blob = EXE.read_bytes()
    seams = [b"CU_API_BASE", b"CU_TOKEN_URL", b"widget_state.json", b"fake_taskbar.txt", b"fake_fullscreen.flag"]
    present = [x.decode() for x in seams if x in blob]
    check("shipped claude-usage.exe carries no test seam (endpoint override / widget dump / fake taskbar)", not present, ", ".join(present))

    # ---- sandbox config ----
    cfg = {"profiles": [{"name": "claude-test", "path": str(PROF)}], "autostart": False, "widget_enabled": True,
           "notify_enabled": False, "widget_cx": 700, "widget_cy": 500, "block_enabled": True, "warn_pct": 101, "block_pct": 102}
    (DATA / "config.json").write_text(json.dumps(cfg), "utf-8")

    # ---- start hidden ----
    p = subprocess.Popen([str(EXE), "--hidden"], env=env()); procs_started.append(p)
    tray = wait_for(lambda: [w for w in windows("ClaudeUsageTray") if w[1] == p.pid], 15)
    check("tray window created by the packaged exe", bool(tray))
    check("daemon.pid holds the exe's pid", wait_for(lambda: (DATA / "daemon.pid").exists() and (DATA / "daemon.pid").read_text().strip() == str(p.pid), 5))
    cache = wait_for(lambda: read_json(DATA / "usage_cache.json"), 10)
    pr = (cache or {}).get("profiles", {}).get("claude-test", {})
    check("first cache written with the sandbox profile (no credentials -> error entry, no usage numbers)", bool(cache) and "claude-test" in cache.get("profiles", {}) and "five" not in pr, json.dumps(pr, ensure_ascii=False)[:100])
    check("no dashboard window on hidden start", not windows("ClaudeUsageDashboard"))
    wd = wait_for(lambda: [w for w in windows("ClaudeUsageWidget") if w[1] == p.pid and w[2]], 10)
    check("taskbar widget window is visible (found by pid)", bool(wd))
    if wd:
        r = wt.RECT(); u.GetWindowRect(wd[0][0], ctypes.byref(r))
        check("widget has a real size", (r.right - r.left) > 20 and (r.bottom - r.top) > 20, f"{r.right-r.left}x{r.bottom-r.top}")
        u.PostMessageW(wd[0][0], 0x8000 + 21, 0, 0)       # the e2e state-dump message: must be ignored by the shipped widget
        time.sleep(0.6)
        check("shipped widget ignores the test state-dump message (no widget_state.json)", not (DATA / "widget_state.json").exists())

    # ---- single instance + dashboard via second launch ----
    t1 = time.time(); s = subprocess.Popen([str(EXE)], env=env())
    try: s.wait(timeout=15)
    except subprocess.TimeoutExpired: s.kill()
    check("second launch exits quickly and successfully (single instance)", s.returncode == 0, f"code={s.returncode} {time.time()-t1:.1f}s")
    check("pid file still the first instance's", (DATA / "daemon.pid").read_text().strip() == str(p.pid))
    dwin = wait_for(lambda: [w for w in windows("ClaudeUsageDashboard") if w[2]], 20)
    check("second launch opens the dashboard window", bool(dwin), f"{time.time()-t1:.1f}s")
    dpid = dwin[0][1] if dwin else None
    check("dashboard runs from the packaged dashboard exe", dpid in [pid for pid, n in procs() if n.lower() == "claude-usage-dashboard.exe"])
    s2 = subprocess.Popen([str(EXE)], env=env()); s2.wait(timeout=15); time.sleep(2)
    check("launching again does not open a second dashboard", len(windows("ClaudeUsageDashboard")) == 1, f"{len(windows('ClaudeUsageDashboard'))} window(s)")
    if dwin:
        u.PostMessageW(dwin[0][0], WM_CLOSE, 0, 0)
        gone = wait_for(lambda: not windows("ClaudeUsageDashboard") and dpid not in [pid for pid, _ in procs()], 10)
        check("closing the dashboard (WM_CLOSE) ends its process", gone)
    check("tray survives the dashboard closing", p.poll() is None and bool([w for w in windows("ClaudeUsageTray") if w[1] == p.pid]))
    # ---- shipped dashboard exe must not carry the e2e hook ----
    # (tray is up, but the dashboard exe is launched directly with --e2e; with the hook the window would stay hidden
    #  and write the result file, without it the flag is ignored: window shows, no file)
    js, out = WORK / "probe.js", WORK / "probe.out"; js.write_text("return 1", "utf-8")
    d = subprocess.Popen([str(DASH), "--e2e", str(js), str(out)], env=env()); procs_started.append(d)
    shown = wait_for(lambda: [w for w in windows("ClaudeUsageDashboard") if w[1] == d.pid and w[2]], 20)
    time.sleep(2)
    check("shipped dashboard exe ignores --e2e (window shows, no result file)", bool(shown) and not out.exists())
    if shown: u.PostMessageW(shown[0][0], WM_CLOSE, 0, 0)
    wait_for(lambda: d.poll() is not None, 10)
    if d.poll() is None: d.kill()

    # ---- hook mode (separate sandbox, fake cache, like Claude Code: stdin json, stderr message) ----
    hk = WORK / "hk"; X = WORK / "X"; X.mkdir(); (hk / "claude-usage").mkdir(parents=True)
    def hook(cache_obj):
        f = hk / "claude-usage" / "usage_cache.json"
        f.write_text(json.dumps(cache_obj), "utf-8") if cache_obj else f.unlink(missing_ok=True)
        t = time.time()
        r = subprocess.run([str(EXE), "hook"], input=b"{}", capture_output=True, timeout=60,
                           env=env({"LOCALAPPDATA": str(hk), "CLAUDE_CONFIG_DIR": str(X)}))
        return r, time.time() - t
    hc = lambda f, w: {"ts": time.time(), "block_enabled": True, "block_pct": 95,
                       "profiles": {"me": {"path": str(X), "five": f, "week": w}, "other": {"path": str(WORK / "Y"), "five": 5, "week": 5}}}
    r_ok, t_ok = hook(hc(10, 10)); r_bl, t_bl = hook(hc(97, 10)); r_no, t_no = hook(None)
    stale = hc(97, 10); stale["ts"] = time.time() - 3 * 3600; r_st, _ = hook(stale)
    check("hook: allows below threshold, no output", r_ok.returncode == 0 and r_ok.stderr == b"" and r_ok.stdout == b"")
    msg = r_bl.stderr.decode("utf-8", "replace")
    check("hook: blocks above threshold (exit 2) with a Vietnamese message naming the other profile", r_bl.returncode == 2 and "Đổi sang" in msg and "'other'" in msg, msg.strip()[:100])
    check("hook: fails open with no cache", r_no.returncode == 0)
    check("hook: fails open on a stale cache", r_st.returncode == 0, f"code={r_st.returncode}")
    check("hook latency < 1 s per call (windowed exe)", max(t_ok, t_bl, t_no) < 1.0, f"{t_ok*1000:.0f} / {t_bl*1000:.0f} / {t_no*1000:.0f} ms")

    # ---- quit from the menu command ----
    h = tray[0][0] if tray else None
    if h: u.PostMessageW(h, WM_COMMAND, ID_QUIT, 0)
    check("Quit: tray process exits with code 0", wait_for(lambda: p.poll() is not None, 10) and p.returncode == 0, f"code={p.returncode}")
    check("Quit: pid file removed, no tray/widget window left", not (DATA / "daemon.pid").exists() and not [w for w in windows("ClaudeUsageTray")] and not [w for w in windows("ClaudeUsageWidget") if w[1] == p.pid])
    time.sleep(1)
    check("nothing of the package is still running", not procs(), str(procs()))
    log = (DATA / "events.log").read_text("utf-8", "replace") if (DATA / "events.log").exists() else ""
    check("events.log has no panic / crash lines", "panic" not in log.lower(), log.strip().splitlines()[-1][:100] if log.strip() else "empty")
finally:
    kill_ours()
    time.sleep(1)
    shutil.rmtree(WORK, ignore_errors=True)

print(f"\n{sum(results)}/{len(results)} checks passed")
sys.exit(0 if results and all(results) else 1)
