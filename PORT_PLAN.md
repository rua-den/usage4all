# PORT_PLAN: claude-usage Python → Rust (kèm refactor dashboard)

Ngày lập: 2026-11-19. Code Python gốc: commit `dd84ff1`, tag `python-1.1.0`.
Đọc `HANDOFF.md` (nhất là mục 6 "bất biến" và mục 9 "bẫy") trước khi làm bất cứ phase nào. Plan này **không** thay thế các bất biến đó.

## 0. Mục tiêu / không-mục-tiêu

Mục tiêu
- Hết lag: hook < 50ms (hiện ~0.5s do onefile giải nén), mở exe/dashboard không còn bước giải nén `_MEI`,
  widget hover/spinner mượt (hiện Pillow supersample x3 mỗi frame 110ms, tỉ lệ với số profile), dashboard phản hồi tức thì.
- RAM tiến trình nền < 20MB (hiện ~63MB). Exe < 15MB.
- **Tương thích dữ liệu 100%**: cùng thư mục `%LOCALAPPDATA%\claude-usage\`, cùng `config.json` / `usage_cache.json` / `alerts.json` / file cờ,
  cùng tên exe và lệnh hook (`"<exe>" hook`) để hook đã cài trong `settings.json` vẫn chạy. Chuyển bản = thay exe.
- Refactor dashboard: bỏ các workaround của pywebview (bridge-ready proxy, tự resize frameless, `_window` riêng tư, ctypes dùng chung),
  chia `ui.html` 1 file 28KB thành component, đổi poll 2s sang **đẩy sự kiện** khi cache/config đổi.

Không-mục-tiêu (giữ nguyên hành vi, làm sau nếu muốn)
- Không đổi UX/giao diện widget (5 kiểu, kéo/resize/xoay, thẻ hover) và giao diện terminal của dashboard (màu, font, bố cục).
- Không thêm tính năng mới trong lúc port (mục 12 HANDOFF để sau).
- Không đổi IPC sang socket/pipe: vẫn qua file (đơn giản, debug được, test e2e cũ dùng lại được).
- Không ký số / installer trong đợt này.

## 1. Đo trước (Phase 0, ~0.5 ngày): xác nhận lag ở đâu

Ghi số liệu vào `bench/BASELINE.md` để so sau port:
1. Hook: `Measure-Command { echo '{}' | .\dist\claude-usage.exe hook }` × 20 lần, lấy median/p95.
2. Mở dashboard: thời điểm chạy exe lần 2 → cửa sổ hiện (log timestamp trong `dashboard.py`, hoặc đo `FindWindow` theo vòng lặp).
3. Widget: thời gian `render()`+`compose()`+`present()` mỗi frame với 4 và 14 profile, mỗi kiểu, khi hover/spinner (thêm timer tạm, không commit).
4. CPU/RAM nền idle 10 phút (Process Explorer / `Get-Process`).
5. Dashboard: thời gian từ click → phản hồi (save, đổi tab, refresh) qua `performance.now()` trong JS.

Quyết định sau Phase 0: nếu lag chỉ là hook + khởi động, có thể dừng ở Phase 2 (hook Rust riêng) + build Python `--onedir`.
Người dùng đã chọn port toàn bộ, nhưng số đo vẫn cần để chứng minh kết quả.

## 2. Kiến trúc đích

```
claude-usage.exe  (1 binary Rust, subsystem windows, manifest: PerMonitorV2 DPI, Common Controls v6)
├─ (không tham số) / --hidden   → daemon     : tray + widget + poller (KHÔNG link WebView2 ở đường này)
├─ dashboard                    → dashboard  : Tauri v2, tiến trình riêng, sống ngắn (giữ quyết định "trả RAM khi đóng")
├─ hook                         → hook       : đọc cache, exit 0/2, ghi stderr. Không khởi tạo gì khác.
└─ preview <out.png> [...]      → dev tool   : render widget ra PNG (thay tools/preview_*.py), dùng cho golden test
```

Workspace Cargo (đặt trong `rust/` cho tới lúc chuyển hẳn; Python vẫn ở gốc repo để so sánh song song):

```
rust/
├─ Cargo.toml                 workspace
├─ crates/core/               KHÔNG phụ thuộc GUI. paths, config (+DEFAULTS, clamp), cache (atomic write+retry), log,
│                             fetch, token refresh, lock, backoff/scheduler (logic thuần), alerts (logic thuần),
│                             fmt_reset/ago/iso, aliases, effective_interval, hookinstall, autostart (winreg)
├─ crates/widget/             Win32 layered window + renderer (Direct2D/DirectWrite) + layout (plan_layout…)
├─ crates/daemon/             App: poll thread, tray, toast, flag files, singleton, mở dashboard
├─ crates/dashboard/          Tauri v2 app (commands + events), frontend ở dashboard/ui/
├─ app/                       bin `claude-usage`: main.rs điều phối subcommand
└─ dashboard/ui/              frontend (Vite + Svelte 5 + TS), build ra dist/ nhúng vào binary
```

### Lựa chọn crate (ghim phiên bản cụ thể trong Cargo.lock)
| Việc | Crate | Ghi chú |
|---|---|---|
| JSON | `serde`, `serde_json` | Struct có `#[serde(default)]` + `flatten` giữ key lạ để **không làm mất key** khi ghi lại config |
| HTTP | `ureq` + `rustls-platform-verifier` (tin kho chứng chỉ Windows) + đọc proxy hệ thống | urllib của Python có sẵn cả 2; thiếu là hỏng sau proxy soi TLS của công ty. Đồng bộ, timeout 20s / 30s như Python. Thử trên mạng công ty |
| Win32 | `windows` (windows-rs) | Window, layered, D2D/DWrite, Shell_NotifyIcon nếu cần, registry có thể dùng `winreg` |
| Tray | `tray-icon` + `muda` (menu) | Chạy trên luồng có message loop; **cùng luồng với widget** (xem 2.1) |
| Toast | `tauri-winrt-notification` hoặc `winrt-notification` | AUMID: cần đăng ký shortcut/AppUserModelID; nếu phiền thì fallback balloon qua `Shell_NotifyIcon` (pystray hiện dùng balloon) |
| Dashboard | `tauri` v2 | `decorations:false`, `visible:false` lúc tạo, `startResizeDragging`, `data-tauri-drag-region` |
| Watch file | `notify` | Dashboard đẩy event khi `usage_cache.json`/`config.json` đổi (debounce 150ms) + poll dự phòng 5s |
| Thời gian | `time` hoặc `chrono` | Parse ISO có offset như `datetime.fromisoformat` |
| Hash | `sha2` | Fingerprint token chết (chỉ trong RAM) |
| Test | `cargo test`, `tiny_http` (server giả), `image` (so golden PNG) | |

### 2.1 Luồng trong daemon
- **Luồng UI (main)**: message loop Win32 duy nhất: cửa sổ widget + tip + tray icon (`tray-icon` cần loop) + timer (`SetTimer` 1s / 110ms).
  Gộp tray vào cùng luồng với widget (Python đang dùng 2 luồng: pystray + widget) → bớt đồng bộ.
- **Luồng poller**: vòng 1s y hệt `app.loop()` (đọc config khi mtime đổi, file cờ, tối đa 1 request/tick, flush toast).
  Gửi kết quả sang luồng UI bằng `PostMessage(WM_APP+n)` (cập nhật icon tray + vẽ lại widget ngay, không chờ tick).
- State chia sẻ: `Arc<Mutex<State>>` nhỏ (data, busy, cfg). Ghi cache vẫn ra file (dashboard + hook đọc).
- Singleton: **giữ `daemon.pid`** (tương thích + test cũ) **và** thêm named mutex `Local\claude-usage-daemon` để chống race 2 lần khởi động cùng lúc.
  Lần chạy thứ 2: ghi `show.flag` rồi thoát (như cũ).

## 3. Kế hoạch theo phase

Mỗi phase kết thúc bằng: test xanh + commit + cập nhật mục "Trạng thái" cuối file này. Ước lượng là làm cùng agent.

### Phase 1: Khung + core dữ liệu (~1 ngày)
- `cargo new` workspace, `rust-toolchain.toml` (stable, target `x86_64-pc-windows-msvc`), `.cargo/config.toml` static CRT (`+crt-static`),
  profile release: `lto="thin"`, `codegen-units=1`, `strip=true`, **`panic="unwind"`** (R3: `catch_unwind` quanh mỗi tick poll, mỗi thân wndproc, và `main` của hook; panic trong hook → exit 0).
- `core::paths`: `DATA = %LOCALAPPDATA%\claude-usage` (đọc env mỗi lần khởi động để test cô lập bằng `LOCALAPPDATA` như Python).
- `core::config`: struct `Config` khớp `DEFAULTS` (mọi key ở HANDOFF mục 5), `load()` = defaults + file + tự phát hiện profile nếu rỗng + `interval_sec >= 90`;
  key không biết thì **giữ lại** khi ghi (`#[serde(flatten)] extra: Map`). `save()` atomic. Kiểu sai trong file (vd chuỗi thay số) → dùng mặc định, không panic.
- `core::cache`: struct theo schema HANDOFF mục 5; `write` atomic với retry khi `ERROR_ACCESS_DENIED`/`SHARING_VIOLATION` (bẫy 6); `load` cũng retry.
- `core::log`: `events.log` tự cắt >200KB, không bao giờ panic.
- `core::fmt`: `fmt_reset`, `ago_text`, `fmt_when`, `money`, `alias/aliases`, `effective_interval`.
- **Test đối chứng (parity)**: script Python `tools/gen_fixtures.py` (dùng chính `core.py`) sinh `rust/fixtures/*.json`
  (config đầu vào → config sau load; ISO/now → chuỗi fmt; profile list → aliases). Rust test đọc fixture và so khớp.
  Chạy lại script mỗi khi đổi logic Python (sau port thì đóng băng fixture).

### Phase 2: Hook (~0.5 ngày) → có thể phát hành riêng ngay
- `hook` subcommand: port `promptguard.run()` từng dòng: cache thiếu/hỏng/`ts` cũ >900s/`block_enabled` tắt/profile không có → exit 0;
  so sánh đường dẫn theo `normcase(abspath)` = `GetFullPathNameW` (xử lý `..`, `.`, đường dẫn tương đối, `/`) rồi lowercase; **không** resolve symlink.
  Mặc định `%USERPROFILE%\.claude`. Thiếu key nào cũng exit 0. `{lim}` in giống Python (int). Profile "best" = min **đầu tiên theo thứ tự trong file** (cần `preserve_order`, lưu ý feature này bật cho cả crate).
  Thông báo chặn y hệt chuỗi Python, ghi UTF-8 thẳng vào `GetStdHandle(STD_ERROR_HANDLE)` bằng `WriteFile` (subsystem windows, bẫy 5).
- Đường `hook` không được chạm tới Tauri/WebView2/tray: `main.rs` match subcommand **trước** mọi khởi tạo. Kiểm tra bằng đo thời gian
  và `dumpbin /dependents` (WebView2Loader phải load trễ hoặc tĩnh nhưng không khởi tạo).
  Nếu binary chung vẫn > 50ms do kích thước/DLL: tách `claude-usage-hook.exe` nhỏ và cho `claude-usage.exe hook` exec nó? → không, đơn giản hơn:
  giữ subcommand trong binary chung, đo; chỉ tách nếu đo thấy chậm (khi đó `hookinstall` phải chuyển hook cũ sang exe mới, cần hỏi người dùng).
- Test: port các case hook trong `test_unit.py` (chạy bằng subprocess, kiểm exit code + stderr UTF-8).
- Tuỳ chọn phát hành sớm: bản Python 1.1.x + exe hook Rust (cần đổi lệnh hook → hỏi người dùng trước khi sửa `settings.json` thật).

### Phase 3: Fetch + làm mới token (~1 ngày, RỦI RO CAO NHẤT)
Port `core.fetch`, `parse_extra`, `parse_scoped`, `refresh_if_expired`, `_acquire/_release`, `_post_refresh`, `_write_creds` **sát từng nhánh**:
- Header usage: `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20`. Lỗi HTTP → `{err, status, retry_after}` như Python (đọc `Retry-After` dạng số).
- Token: chỉ khi `expiresAt` (ms) đã qua và có `refreshToken`. `refreshTokenExpiresAt` đã qua hoặc fingerprint nằm trong `_dead` → DEAD_MSG, không POST.
- Khoá: `mkdir` 2 thư mục (`<profile>\.oauth_refresh.lock`, `<realpath profile>.lock`), stale > 90s → chiếm (rmdir+mkdir), thất bại → `retry_after=30`.
  `realpath` dùng `std::fs::canonicalize` rồi chuyển `\\?\UNC\x` → `\\x` và `\\?\C:` → `C:`; `canonicalize` lỗi (đường dẫn không tồn tại) → dùng đường dẫn gốc
  (Python 3.14 `realpath` không ném lỗi). Test với junction/symlink/UNC, so với Python trên cùng đường dẫn.
  Release theo thứ tự ngược, bằng RAII guard (`panic=unwind` nên guard chạy cả khi panic; vẫn ưu tiên mọi lỗi là `Result`).
  **Giữ khoá sống**: Python không cập nhật mtime khoá, mà POST có thể mất 2×30s > ngưỡng stale 60s của CLI → CLI có thể chiếm khoá giữa chừng.
  Bản Rust cập nhật mtime thư mục khoá mỗi 5s trong lúc giữ (như proper-lockfile), và giới hạn tổng thời gian POST < 50s.
- Trong khoá: đọc lại file; nếu `refreshToken` đổi hoặc chưa hết hạn → thôi. Ghi `.credentials.json.claude-usage.bak` **trước** POST.
- POST JSON body `grant_type/refresh_token/client_id/scope`; `invalid_scope` → thử lại 1 lần không `scope`; `invalid_grant` → dead.
  Mã lỗi OAuth chỉ lấy nếu khớp `^[a-z_]{1,40}$` (`error` có thể là string hoặc `{type}`); **không bao giờ** đưa body vào log/err.
- Ghi: giữ mọi key khác (`serde_json::Value`, không struct cứng), JSON gọn `separators=(",",":")` + `ensure_ascii=False`
  (serde_json mặc định đã gọn và không escape non-ASCII: khớp). Thứ tự key: dùng feature `preserve_order` của serde_json.
  Atomic: tmp `<name>.<pid>.claude-usage.tmp` + `MoveFileExW(REPLACE_EXISTING)` retry 50×20ms, cuối cùng ghi thẳng.
- `expiresAt = now_ms + expires_in*1000`; `scopes` từ `scope` nếu có; `refreshTokenExpiresAt = min(cũ, now+rte_in)` chỉ khi cả hai là số.
- Test (port + mở rộng từ `test_unit.py`, server giả `tiny_http` trên 127.0.0.1, URL inject qua biến môi trường **chỉ ở build test** (`cfg(test)`/feature `test-endpoints`),
  bản release không có đường đổi endpoint): còn hạn → không POST; hết hạn → ghi đúng trường + giữ key lạ + .bak; xoay vòng refresh token;
  invalid_grant → dead + không gửi lại tới khi file đổi; invalid_scope → retry không scope; khoá đang giữ → retry 30; khoá stale → chiếm;
  CLI refresh trong lúc chờ khoá → không POST; file đang mở bởi reader → vẫn ghi được; body lỗi chứa "secret" không lọt vào err.
- Kiểm tra thủ công cuối phase (hỏi người dùng trước): chạy với 1 profile thật có token đã hết hạn, so `.credentials.json` trước/sau (chỉ so cấu trúc/key, không in giá trị).

### Phase 4: Daemon: poller, thông báo, tray (~1.5 ngày)
- `scheduler` (logic thuần, test được không cần mạng): `seed` từ cache (không bắn API ồ ạt; không cache → lệch 4s/profile), `next_at` theo profile,
  1 request/tick, backoff: 429 → `Retry-After` hoặc `min(900, 120·2^(n-1))`; status 0 + retry_after → đúng số đó; 401/credentials → `max(iv,300)`;
  khác → `min(600, iv·2^(n-1))`; sàn 30s; jitter +0–10%. Lỗi giữ số cũ + `err/status/retry_at`. Đổi config → tính lại `next_at`.
  Refresh thủ công: bỏ qua nếu fetch <20s hoặc trong cửa sổ 429; ghi `refresh.started/skipped` với đúng chuỗi lý do tiếng Việt.
  Inject đồng hồ + hàm fetch (trait) để test.
- `alerts` (logic thuần): khoá `"<account>|<five|week>|<warn|block>"` → reset ts, dung sai 30 phút, baseline lần đầu (không có `alerts.json`)
  tới khi đủ mọi account hoặc 2 phút, luôn ghi trạng thái kể cả khi tắt thông báo, chỉ xét dữ liệu không `err`.
  Gộp: chờ 6s, cách nhau ≥15s, ≤4 dòng + "+N account khác", dòng "Nên dùng: …", tiêu đề "CHẶN"/"sắp hết", cắt 250 ký tự.
  Port **toàn bộ** test thông báo từ `test_unit.py` (đây là chỗ từng lỗi "noti hoài").
- Tray: icon vẽ số % cao nhất + màu (port `App.make_icon`, vẽ bằng D2D hoặc tiny-skia ra HICON), trạng thái busy, menu giống pystray hiện tại
  (đọc `app.py` để lấy đúng mục + thứ tự), double-click mở dashboard.
- Toast: thử WinRT toast trước; cần AUMID → đăng ký trong registry HKCU (không cần shortcut) hoặc dùng AUMID của PowerShell như nhiều app nhỏ.
  Nếu WinRT rắc rối: balloon `Shell_NotifyIconW(NIF_INFO)` như pystray (hành vi hiện tại, chắc chắn chạy).
- File cờ: `show.flag` → mở/đưa dashboard lên trước (port `App.show/_show_locked`: tìm cửa sổ dashboard theo class/title, `SetForegroundWindow`,
  chờ tối đa 2.5s nếu tiến trình dashboard còn sống mà chưa có cửa sổ), `notify_test.flag`, `refresh.flag`.
- Quit: xoá `daemon.pid`, kill tiến trình dashboard con (Job Object `KILL_ON_JOB_CLOSE` thay `taskkill /T`).
- Autostart: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` giá trị như `core.set_autostart`/`launch_cmd` (cùng tên value để ghi đè bản Python).

### Phase 5: Widget (~3 ngày, phần lớn nhất)
Port `widget.py` (941 dòng). Tách 3 lớp để test được:
1. **Layout thuần** (`visible_profiles`, `sort_profiles`, `stack_limit`, `plan_layout` + ô `+N` (`MORE`), `apply_plan`, `tip_position`, `shows/metrics/reset_lines`,
   `pct_color`, `cell_fill`, `font_scale`, `opacity_alpha`, hit-test, xoay toạ độ). Hàm `measure` inject được → test không cần GPU.
   Parity test với fixture sinh từ Python (đầu vào cfg/cache/scale → plan).
2. **Renderer**: Direct2D vẽ vào `ID2D1DCRenderTarget`/WIC bitmap premultiplied BGRA → `UpdateLayeredWindow`.
   Không cần supersample (D2D antialias). Font Segoe UI / Segoe UI Bold qua DirectWrite. 5 kiểu: `rings`, `bars`, `cards`, `segments`, `arc`;
   overlay chỉnh sửa + xoay (`compose`), spinner (phase), plate nền mờ, theme sáng/tối (`light_taskbar` đọc registry), opacity.
   **Lưu ý bẫy 3**: D2D blend đúng alpha nên vấn đề "ImageDraw ghi đè pixel" biến mất, nhưng thứ tự vẽ marker "đang fetch" sau nền thẻ vẫn giữ.
   Cache: giữ `plan_key` (bẫy 13); cache thêm layer tĩnh (nền + chữ) và chỉ vẽ lại spinner mỗi frame → mục tiêu < 2ms/frame với 14 profile.
3. **Window/input**: class `ClaudeUsageWidget` + `ClaudeUsageTip` (`WS_EX_LAYERED|TOOLWINDOW|TOPMOST|NOACTIVATE`, tip thêm `TRANSPARENT`),
   chuột: kéo/resize/xoay/khoá, `TrackMouseEvent` hover/leave, menu chuột phải (đọc `Widget.menu`), timer 1s/110ms,
   taskbar ngang/dọc (`SHAppBarMessage`), `auto_center` neo trái tray hoặc phía trên khay, tự ẩn khi fullscreen (`SHQueryUserNotificationState`)
   hoặc taskbar ẩn, DPI per-monitor (`WM_DPICHANGED`), `sig` đầy đủ mọi key config ảnh hưởng (checklist HANDOFF mục 13).
- So sánh hình: `claude-usage preview` render ma trận (5 kiểu × layout ngang/dọc × hover/không × spinner × 1/4/14 profile × sáng/tối) ra PNG;
  `tools/preview_*.py` của Python render cùng ma trận. Không so pixel chính xác (rasterizer khác) mà: (a) so **bounding box/kích thước ảnh** khớp ±1px,
  (b) người dùng duyệt bằng mắt một trang HTML ghép đôi Python|Rust (sinh tự động, mở cục bộ). Không chụp màn hình thật (bẫy 11).
- e2e: `tests/e2e_widget.py` hiện **import `widget` và chạy trong tiến trình test**, đọc `w.geo/org/img/rot/built_angle` → **phải viết lại**.
  Thêm kênh soi trạng thái chỉ có trong build feature `e2e` (dump JSON hình học + PNG ảnh hiện tại ra file khi nhận `WM_COPYDATA`),
  và cờ thay `TrackMouseEvent` thật cho test hover (bẫy 9).

### Phase 6: Dashboard refactor + Tauri (~2.5 ngày)
**Backend (Rust, `crates/dashboard`)** – commands thay `dashboard.Api`:
| pywebview `Api` | Tauri | Ghi chú |
|---|---|---|
| `state()` | `get_state` + event `state-changed` | Event đẩy khi file cache/config đổi (`notify`), poll dự phòng 5s. Dữ liệu ban đầu vẫn nhúng vào trang (`window.__INIT__` qua `initialization_script`) để hiện số ngay |
| `save(v)` | `save_config(patch)` | Load-modify-save trong 1 mutex (thay `CFG_LOCK`); **giữ ngữ nghĩa Python**: `cfg.update(v)`, chỉ clamp `interval_sec` (phần clamp còn lại nằm ở frontend), ghi cả config đã merge (kể cả profile tự phát hiện), gọi `set_autostart` mỗi lần save. Không thêm whitelist |
| `refresh`, `test_notify` | giữ | Ghi file cờ |
| `hook_status/install/uninstall` | giữ | Gọi `core::hookinstall` |
| `widget_unlock`, `widget_reset` | giữ | Sửa config |
| `hide`, `minimize` | `hide_and_quit`, `minimize` | Ẩn ngay (`window.hide()`), lưu cỡ, rồi **`TerminateProcess(GetCurrentProcess())`**; không dùng `std::process::exit` (= ExitProcess, đường chậm theo bẫy 16). `CloseRequested` (Alt+F4) → `prevent_close` + cùng đường nhanh. Thư mục data WebView2 đặt ở `DATA\webview\<pid>` (mặc định của Tauri nằm ngoài `claude-usage`) + dọn ở lần mở sau |
| `rect/set_rect` + 8 div resize | **bỏ** | `appWindow.startResizeDragging(dir)` (native, mượt). Giữ 8 vùng cạnh ở CSS chỉ để gọi hàm này |
| `toggle_max`, `work_area` | `toggleMaximize()` native | Frameless maximize của Tauri tôn trọng work area; kiểm tra lại với taskbar dọc bên phải |
| `fit(h)`, `auto_fit`, `win_state`, `page_ready` | `fit_height(h)`, `auto_fit`, `win_state`, `ready` | Giữ ngữ nghĩa `user_sized`/`maximized`; cửa sổ tạo `visible:false`, hiện khi trang báo `ready` (timeout 4s) |
| `_persist_now/_soon`, `restore_saved` | giữ, nghe `WindowEvent::Moved/Resized` debounce 800ms | `dash_rect` vẫn [x,y,w,h] **pixel vật lý** (đọc chấp nhận float). Khi maximized phải lưu **rect thường do mình tự theo dõi**: không dùng outer size lúc max, không dùng `rcNormalPosition` (toạ độ workspace lệch khi taskbar trái/trên). Port `initial_width` theo số profile, MIN 420×330, kẹp on-screen, kẹp fit = cao màn hình − 40·scale |

**Frontend (`dashboard/ui`, Vite + Svelte 5 + TypeScript)**:
- Chia `ui.html` thành: `TitleBar`, `Accounts` (+ `AccountRow`, `ProgressBar`, `LiveText` cho ago/until/reset), `Settings` (`GeneralTab`, `LookTab`, `AliasEditor`),
  `Footer`, `Toast`, `ResizeHandles`; `store.ts` (state + cfg), `api.ts` (wrapper `invoke` có kiểu), `format.ts` (port `bar/reset/ago/until/tipText`), `fit.ts`.
- Svelte cập nhật theo khoá (`{#each … (name)}`) → bỏ các hack chống chớp (`setHTML`, `fillLive`, so chuỗi `innerHTML`) mà vẫn không chạy lại animation.
  Text đổi theo thời gian dùng 1 store `now` tick 1s.
- Giữ nguyên CSS hiện tại (copy biến màu, font Cascadia Mono, media query 640/560/460, chế độ `compact`, không thanh cuộn thấy được, `#toast` `pointer-events:none` (bẫy 15)).
- Tự lưu tab "Giao diện widget" (debounce 450ms) và đảm bảo save đang chờ được xả trước khi đóng (`closeDash`).
- Phím tắt `r` / `Esc`, double-click thanh tiêu đề = maximize.
- Bundle nhúng vào binary (`frontendDist`), không cần server khi chạy. Kích thước JS mục tiêu < 60KB.
- Build frontend cần Node (dev-only); CI/`build.ps1` chạy `npm ci && npm run build` trước `cargo build`. Commit `package-lock.json`.
- e2e: `tests/e2e_dashboard.py` + `e2e_responsive.py` hiện điều khiển qua `evaluate_js` của pywebview → **phải viết lại**:
  chọn (a) WebDriver cho Tauri (`tauri-driver` + msedgedriver) hoặc (b) cờ debug `--e2e <script>` chạy kịch bản JS trong trang và ghi kết quả ra file.
  Đề xuất (b): không thêm phụ thuộc, chạy offline; cờ chỉ có trong build debug/feature `e2e`.

### Phase 7: Đóng gói, chuyển đổi, dọn dẹp (~1 ngày)
- `build.ps1` mới: unit test (`cargo test --workspace`) → `npm ci && npm run build` → `cargo build --release` → nhúng icon + version info (`winres`/`tauri-build`,
  version lấy từ `Cargo.toml` workspace) → zip + SHA-256 (.NET, bẫy 7) vào `release\`. Phiên bản mới: `2.0.0`.
- `tests/smoke_release.py`: đổi để chạy gói Rust (giải nén zip, khởi động, widget, dashboard, 1 instance, hook, credit parse thật).
- WebView2 Runtime: Windows 11 có sẵn; nếu thiếu → dashboard hiện MessageBox hướng dẫn thay vì crash (daemon/hook không cần WebView2).
- Chuyển đổi trên máy người dùng (**hỏi trước**): tắt `claude-usage.exe` Python, thay `dist\claude-usage.exe`, mở lại `--hidden`.
  Config/cache/alerts dùng tiếp; autostart trỏ cùng đường dẫn; hook trong `settings.json` cùng lệnh → không phải cài lại.
  Rollback: `git checkout python-1.1.0` + build lại, hoặc giữ sẵn exe Python 1.1.0 trong `release\`.
- Chạy song song 1–2 ngày (chỉ bản Rust chạy; không chạy 2 daemon cùng lúc vì cộng dồn hạn mức API, bẫy 12).
- Sau khi ổn: chuyển Python vào `legacy/` (hoặc xoá, vẫn còn trong tag), cập nhật `README.md`, `CHANGELOG.md`, `HANDOFF.md`
  (kiến trúc, bản đồ file, lệnh test, bẫy mới, mục 11 ghi lại vì sao đổi quyết định), `CLAUDE.md` (lệnh chính).

## 4. Chiến lược test tổng
| Lớp | Cách | Nguồn |
|---|---|---|
| Logic thuần (config, fmt, scheduler, alerts, layout) | `cargo test`, fixture sinh từ Python | port 67 test của `test_unit.py` |
| Token refresh / fetch | `cargo test` + server giả cục bộ | như trên |
| Hook | subprocess thật trên exe release | như trên |
| Renderer | `preview` ra PNG + trang so sánh bằng mắt + kiểm kích thước | `tools/preview_*.py` |
| Widget thật | `tests/e2e_widget.py` (đổi đường dẫn exe) | giữ |
| Dashboard thật | viết lại theo Phase 6 | `e2e_dashboard.py`, `e2e_responsive.py` |
| App nền thật (API thật) | `tests/e2e_app.py` (đổi đường dẫn exe) | giữ |
| Gói | `tests/smoke_release.py` | giữ, chỉnh |
| Hiệu năng | lặp lại các phép đo Phase 0, ghi `bench/AFTER.md` | mới |

Quy tắc giữ nguyên: test luôn đặt `LOCALAPPDATA` sang thư mục tạm; **một** test GUI mỗi lần; token giả `"test-token-not-real"`; không in token.

## 5. Rủi ro & cách giảm
| Rủi ro | Hậu quả | Giảm thiểu |
|---|---|---|
| Sai logic refresh token (xoay vòng) | Profile phải `/login` lại | Port sát từng nhánh, test server giả đủ mọi nhánh, review riêng phase 3, thử thật 1 profile có hỏi trước, `.bak` trước POST |
| Lệch tên thư mục khoá với CLI (`\\?\`, hoa/thường) | Đua với CLI, ghi đè token | Test `realpath` với symlink/junction; so với Python trên cùng đường dẫn |
| Mất key khi ghi config/credentials | Mất cài đặt / hỏng CLI | `Value`/`flatten` giữ key lạ + `preserve_order`; test round-trip |
| Hình widget khác bản cũ | Người dùng không thích | Trang so sánh bằng mắt, người dùng duyệt trước khi chuyển |
| Toast WinRT cần AUMID | Không hiện thông báo | Fallback balloon như hiện tại; luôn ghi `events.log` |
| Tauri frameless trên taskbar dọc/DPI ≠ 100% | Maximize/resize sai | Test e2e responsive + thử tay DPI 125/150% |
| Hook binary chung khởi động chậm | Không đạt <50ms | Đo ở Phase 2; nếu chậm thì tách exe hook (đổi lệnh hook → hỏi người dùng) |
| Thời gian ước lượng vượt | | Mỗi phase dùng được độc lập; Phase 2 có giá trị ngay cả khi dừng |
| Node thành phụ thuộc build | Build phức tạp hơn | Chỉ dev-time; khoá phiên bản; có thể chuyển sang vanilla TS nếu muốn bỏ Svelte |

## 6. Tổng ước lượng
Phase 0: 1–2 · P1: 1.5 · P2: 1 · P3: 2 · P4: 2.5 · P5: 6–8 · P6: 4–5 · P7: 1.5 · viết lại e2e: 3–4 → **~20–28 ngày làm việc** (cùng agent), chưa tính vòng duyệt hình widget.
Bản đầu ghi 11 ngày. Review chỉ ra là thiếu: viết lại toàn bộ e2e, dựng lại 5 kiểu + thẻ hover bằng D2D, và spike Tauri.

## 7. Câu hỏi cần người dùng chốt
1. Frontend dashboard: Svelte 5 (đề xuất) hay TypeScript thuần chia module (ít phụ thuộc hơn, code dài hơn)?
2. Toast: chấp nhận balloon kiểu cũ nếu WinRT toast phiền, hay bắt buộc toast hiện đại?
3. Phiên bản mới đánh số `2.0.0`?
4. Sau khi chuyển: xoá code Python khỏi nhánh chính (vẫn còn ở tag) hay giữ trong `legacy/`?

## 8. Sửa theo review (2026-11-19, reviewer Opus context trống; các điểm [V] đã đối chiếu lại code)

Đã sửa ngay trong các mục trên: panic strategy (R3), giữ khoá sống + realpath UNC (R8), khớp đường dẫn hook (R9), thoát nhanh dashboard bằng TerminateProcess (R4),
lưu rect khi maximized (R13), `save` giữ ngữ nghĩa Python (R7), e2e_widget phải viết lại (R2), TLS/proxy (R5), ước lượng (R14).
Các điểm còn lại, bắt buộc đưa vào phase tương ứng:

**R1 (BLOCKER): `hookinstall.repair()` tự ghi `settings.json` thật mỗi lần daemon khởi động** (`app.py:306-310`, `hookinstall.py:87-99`).
Profile lấy từ HOME thật kể cả khi `LOCALAPPDATA` sandbox. Hậu quả nếu port nguyên:
- chạy bản dev từ `rust\target\release\` hoặc smoke test từ thư mục giải nén tạm → hook thật của người dùng bị trỏ sang exe đó;
- Python và Rust chạy xen kẽ sẽ "giành" hook của nhau.

Đề xuất (cần người dùng chốt):
- (a) build dev/test (`debug_assertions` hoặc feature `e2e`) **không bao giờ** gọi repair;
- (b) bản release chỉ repair khi exe cũ trong lệnh hook **không còn tồn tại**, và ghi log;
- (c) giữ đúng marker: lệnh chứa `claude-usage` và kết thúc bằng ` hook` (`hookinstall.py:17-19`);
- (d) bỏ ý tưởng "phát hành hook Rust riêng" ở P2 trừ khi đặt cùng tên/đường dẫn exe.

**R10: đo độ trễ end-to-end, không chỉ thời gian exe hook.** Claude Code chạy hook qua shell, nên shell có thể chiếm phần lớn thời gian.
P0 phải đo thêm độ trễ prompt khi có và không có hook. Binary chung dùng `/DELAYLOAD` cho d2d1/dwrite/WebView2Loader/ole32 để đường `hook` không phải nạp các DLL đó.

**R11: tray/toast.**
- Mục mặc định của pystray chạy khi **click trái một lần** (`WM_LBUTTONUP`), không phải double-click.
- Port tooltip cắt 127 ký tự, hậu tố tiêu đề "(N account)", và "tắt thông báo thì xoá hàng chờ" (`app.py:126,231,247`).
- Không dùng AUMID của PowerShell (toast sẽ hiện tên "Windows PowerShell"). Đăng ký AUMID riêng ở `HKCU\Software\Classes\AppUserModelId\…`.
  Đây là ghi registry, cần ghi vào tài liệu, và test không được chạm (sandbox `LOCALAPPDATA` không che HKCU).

**R12: renderer** dùng WIC bitmap render target với `D2D1_RENDER_TARGET_TYPE_SOFTWARE`, không dùng hardware DC target.
Lý do: hardware target kéo D3D/driver GPU vào và phá mục tiêu RAM < 20MB.

**R15: hành vi lúc khởi động/poll cần port thêm:**
- khởi động xoá file cờ cũ và ghi lại autostart nếu đang bật (`app.py:22-27`);
- refresh thủ công giãn `now+i*2`;
- ghi `busy` **trước** khi gọi request;
- `Retry-After` chỉ nhận chữ số;
- User-Agent `claude-usage/<ver>` chỉ gửi ở POST token.

**R16:**
- Binary chung vẫn *link* WebView2Loader (chỉ không khởi tạo), nên dùng delay-load.
- `App.dash_window` đang tìm cửa sổ theo class WinForms (`app.py:57`), phải đổi sang class/title của Tauri.
- Chỉ dùng một trong `winres` hoặc `tauri-build` cho resource, tránh trùng manifest.

**R17:** subcommand `preview` chỉ có trong build feature `dev`.

**R6: kiểu config.** `widget_size`, `widget_cx/cy/angle`, `dash_rect` có thể là **float** (`widget.py:826,839`, `dashboard.py:198`).
Deserialize từng field từ `Value`, chấp nhận cả int lẫn float, không ép i32. Kiểu sai thì dùng mặc định cho đúng field đó (Python sẽ ném lỗi, Rust mềm hơn; ghi chú khác biệt này).

**Lag: Phase 0 phải là cổng quyết định.** Reviewer chỉ ra (đã xác nhận trong code) phần lớn lag sửa rẻ được ngay trong Python:
- onefile giải nén `_MEI` ở mỗi lần chạy hook và mỗi lần mở dashboard; `_show` còn chờ tới 2.5s → build `--onedir`;
- mỗi `Widget.tick` (kể cả timer 110ms khi hover) gọi `load_cache()` trên luồng UI, có thể ngủ tới 125ms khi retry, cộng thêm đọc registry `light_taskbar()` và FindWindow;
- kéo resize widget: `size` nằm trong `plan_key`, nên mỗi lần chuột di chuyển lại đo layout bằng cách render supersample x3. Đây nhiều khả năng là chỗ giật nhất.

→ **Phase 0b (1–2 ngày, tuỳ chọn): sửa nhanh bản Python**: `--onedir`, đưa IO ra khỏi luồng UI, cache layout trong lúc kéo, hook native nhỏ.
Đo lại, rồi mới quyết định port toàn bộ hay không. Nếu chọn port, các bản sửa này vẫn đáng làm để có mốc so sánh.

### Quyết định cần chốt trước khi bắt đầu
1. Chính sách `repair` (R1).
2. Làm Phase 0b trước hay port thẳng.
3. Svelte 5 hay TS thuần cho dashboard.
4. Toast: balloon cũ hay WinRT + AUMID riêng (ghi HKCU).
5. Tiêu chí chấp nhận hình widget (duyệt bằng mắt từng kiểu).
6. Phiên bản `2.0.0`; Python để ở `legacy/` hay chỉ giữ trong tag.

### Đã chốt (2026-11-19, người dùng giao cho agent chọn phương án tốt nhất)
1. **Repair**: build dev/test (`debug_assertions` hoặc feature `e2e`) không bao giờ sửa hook. Bản release chỉ sửa khi exe trong lệnh hook cũ **không còn tồn tại**, và ghi log. Giữ nguyên marker.
2. **Không làm Phase 0b đầy đủ.** Làm Phase 0 (đo) để có mốc so sánh. Chỉ thêm 1 sửa rẻ, không đụng code: build Python `--onedir` nếu số đo cho thấy giải nén onefile là thủ phạm,
   để đỡ lag trong lúc port. Không sửa thêm code Python (sẽ bỏ).
3. **Svelte 5 + TypeScript + Vite.**
4. **Toast WinRT + AUMID riêng** (`claude-usage`, ghi HKCU lúc daemon khởi động, giống cách đã ghi autostart; test không chạy đường này). WinRT lỗi thì fallback balloon. Luôn ghi `events.log`.
5. **Hình widget**: trang so sánh Python|Rust do `preview` sinh ra; người dùng duyệt bằng mắt từng kiểu trước khi chuyển bản.
6. **Phiên bản `2.0.0`.** Python giữ ở gốc repo tới khi chuyển bản xong, sau đó xoá khỏi `main` (vẫn còn ở tag `python-1.1.0`).

## Trạng thái
- [x] Git init + tag `python-1.1.0`
- [x] Plan v1 + review độc lập (mục 8) + sửa plan
- [x] Chốt 6 quyết định ở mục 8
- [x] Phase 0: số đo ở `bench/BASELINE.md`; build Python chuyển sang onedir (hook 840 → 232ms), đang chạy thật
- [ ] Phase 1 … Phase 7
- [x] Phase 1 (cu-core + parity fixtures), Phase 2 (hook Rust + hookinstall, R1 repair), đã verify
- [x] Phase 4a: scheduler + alerts + tray state (logic thuần, trait `Env`), `parity_daemon.rs` so với 58 kịch bản Python (~500 bước) qua; clippy/fmt sạch
- [x] Phase 3 (logic): `cu-core/src/net.rs` = fetch + refresh token + khoá mkdir (giữ sống 5s) + ghi atomic, qua trait `Http` (không có đường đổi endpoint). `tests/net.rs` 18 test với server giả, token giả. Khác plan: không dùng `tiny_http`/env var endpoint (trait an toàn hơn). Chưa có: transport thật (`ureq`+rustls) trong `app`, nhánh "đọc lại trong khoá" chưa test được (chỉ review), chưa so với 1 profile thật (cần hỏi người dùng).
- [x] Phase 4b (tray + daemon, KHÔNG gồm widget/dashboard): `app/src/{transport,daemon,icon,tray}.rs`. Tray Win32 (cửa sổ ẩn, icon vẽ bằng bitmap font, menu 6 mục như pystray, click trái = mở dashboard, TaskbarCreated), toast = BALLOON (chưa WinRT/AUMID), autostart registry (chỉ release; CU_NO_AUTOSTART tắt được), repair hook R1, poller chạy worker thread. `app/tests/tray_ui.rs`: UI test tiến trình thật trong sandbox (cửa sổ, pid, singleton lần 2, menu qua WM_COMMAND, click, toggle config, test toast, Quit). Release 2.8MB, hook ~14ms (gồm spawn PowerShell), tray idle ~11MB. Chưa làm: mở dashboard thật (Phase 6), widget (Phase 5), WinRT toast, proxy hệ thống, Job Object. Chưa kiểm chứng bằng mắt: icon trong khay, hình menu, balloon.

- Phase 5 (widget) xong: `app/src/widget/` (renderer phần mềm SDF + GDI text, không D2D), layout parity 1320 ca với Python, `widget_ui` test, lệnh `preview`. Chưa xem bằng mắt trên desktop thật; DPI theo hệ thống (không đổi khi kéo sang màn hình khác); TrackMouseEvent thật chưa test.
- Phase 6 (dashboard) xong theo hướng khác plan: `rust/dashboard` (wry+tao) nhúng `ui.html` + shim `pywebview.api`, KHÔNG Svelte/Tauri. e2e bằng `--e2e` (debug hoặc `--features e2e`). Chưa xem cửa sổ bằng mắt.
- Đã thêm: toast WinRT (AUMID, fallback balloon; chưa xem trên màn hình thật), Job Object, ProxyOverride, `build-rust.ps1` + `BUILD.md`, `tests/smoke_release_rust.py` (28 check). Còn lại: PAC proxy, ký số, cutover (hỏi trước), test DOM/responsive chi tiết của dashboard. Gói tạm: thư mục release/rust-2.0.0-dev (2 exe + SHA256SUMS.txt, phải để cạnh nhau).

### Trạng thái cuối phiên (2026-11-22)
- Phase 5 + 6 + toast WinRT + Job Object + ProxyOverride xong; Phase 7 một phần: `build-rust.ps1` (clippy + test + build + zip + dò seam), `BUILD.md`, `tests/smoke_release_rust.py` (30 check). Gói: `release/claude-usage-rust-v2.0.0-dev-win-x64.zip`. Version vẫn `2.0.0-dev` (đổi ở `rust/Cargo.toml` khi phát hành).
- Test đã port từ e2e Python: dashboard_legacy/responsive/drag, widget_e2e/realmouse, app_e2e (server HTTP giả, CU_API_BASE chỉ có ở debug/feature e2e), dpi_scale. Full test xanh (tray Rust phải thoát, máy mở khoá, không chạm chuột).
- Chưa làm / chưa kiểm chứng: xem giao diện bằng mắt; toast WinRT hiện thật; refresh token thật trên bản Rust; DPI nhiều màn hình; PAC proxy; ký số; cutover chính thức (đổi tên 2.0.0, chuyển Python vào legacy/, cập nhật CLAUDE.md); chưa commit gì (nhánh `rust-port`, toàn bộ là thay đổi chưa commit). Bản Rust đã chạy thật trên máy người dùng ~16 phút: fetch 4 profile ok, log sạch.
- Việc tiếp theo gợi ý: người dùng dùng thử và báo xấu/hỏng; commit nhánh rust-port; chốt version 2.0.0; dọn `release/rust-2.0.0-dev` cũ; cập nhật CLAUDE.md lệnh chính sang build-rust.ps1 khi cutover.
