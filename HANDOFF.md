# HANDOFF: claude-usage

Tài liệu bàn giao cho AI / người tiếp theo. Đọc hết mục 1-4 trước khi sửa code. Cập nhật file này mỗi khi thay đổi kiến trúc,
thêm config key, hoặc phát hiện thêm "bẫy" (mục 9). Ngày cập nhật gần nhất: 2026-11-12, phiên bản `1.1.0` (tự làm mới token).

## 1. Đây là gì

App Windows theo dõi **hạn mức Claude (5h / 7d) của nhiều account Claude Code cùng lúc**. Thư mục `claude-usage.exe` + `_internal` (PyInstaller onedir):

- **Tray icon** (số % cao nhất, đổi màu), **widget trên taskbar** (5 kiểu, kéo thả/resize/xoay, hover ra thẻ chi tiết),
  **dashboard** (cửa sổ kiểu terminal, kéo giãn được), **thông báo toast** (cấu hình được, không spam),
  **hook chặn prompt Claude Code CLI** khi account vượt ngưỡng (tuỳ chọn, mặc định KHÔNG cài).
- Chủ dự án: người dùng cá nhân (máy Windows 11, 4 profile Claude Code gói Team). Giao tiếp bằng **tiếng Việt**, thích câu trả lời ngắn,
  ghét thanh cuộn thấy được, ghét thông báo lặp. Các account đều hợp lệ của họ.
- Ngôn ngữ/stack: Python 3.14, `pystray` + `Pillow` (tray, widget), `pywebview` (WebView2, dashboard), PyInstaller `--onedir` (dồn vào `dist\` để đường dẫn exe không đổi).
  **Đang port sang Rust** (quyết định 2026-11-19, xem mục 11 và `PORT_PLAN.md`). Code Python gốc ở tag `python-1.1.0`.
- Repo git (nhánh `main`, tag `python-1.1.0`), **chưa có remote**: người dùng sẽ tự chọn chỗ push.

### Trạng thái hiện tại

Bản đóng gói gần nhất: `release\claude-usage-v1.1.0-win-x64.zip` (onedir, build 2026-11-19, unit 67/67 + `smoke_release` 13/13), đang chạy thật từ `dist\`.
**Chưa kiểm chứng bằng người thật / môi trường thật** (xem mục 12): click chuột thật vào tray/widget, kéo cạnh dashboard bằng chuột thật,
hook đã cài vào `settings.json` thật của 4 profile (chỉ test trên profile sandbox), exe chưa ký số, máy Windows 10 / máy không có WebView2.

## 2. Bắt đầu nhanh (cho agent)

```powershell
cd D:\stu\repo\claude-usage
.\.venv\Scripts\python tests\test_unit.py          # ~40s, không cần mạng/màn hình/credential thật
.\.venv\Scripts\python main.py                      # chạy từ source (tray + dashboard)
.\.venv\Scripts\python main.py --hidden             # chỉ tray + widget
powershell -ExecutionPolicy Bypass -File build.ps1  # test + build exe + đóng gói zip + sha256 (kill claude-usage.exe đang chạy)
```

- Môi trường: `.venv` đã có sẵn (xem `requirements.txt`). Tạo lại: `python -m venv .venv; .\.venv\Scripts\pip install -r requirements.txt`.
  `node` là tuỳ chọn (một unit test kiểm tra cú pháp JS của `ui.html` nếu có `node`).
- App chạy thật đang nằm ở `dist\claude-usage.exe` (autostart trỏ vào đó). `build.ps1` tắt nó rồi build lại; **mở lại sau khi build**:
  `Start-Process .\dist\claude-usage.exe -ArgumentList "--hidden"`.
- Dữ liệu runtime: `%LOCALAPPDATA%\claude-usage\` (mục 5). Mọi test cô lập bằng cách đặt biến môi trường `LOCALAPPDATA` sang thư mục tạm
  trước khi `import core`. **Không bao giờ để test ghi vào dữ liệu thật** của người dùng.

## 3. Kiến trúc

```
claude-usage.exe + _internal\  (onedir, windowed)
├─ main.py            điều phối theo tham số
│   ├─ (không tham số) / --hidden  → app.main()        : tiến trình NỀN (singleton)
│   ├─ dashboard                   → dashboard.run()   : tiến trình RIÊNG, sống ngắn, chỉ khi mở dashboard
│   └─ hook                        → promptguard.run() : Claude Code UserPromptSubmit hook (exit 0 cho qua / 2 chặn)
└─ Tiến trình nền = App (app.py)
     ├─ thread chính : pystray icon (tray)
     ├─ thread loop  : poll API từng profile, ghi cache, quyết định toast, xử lý các "flag file"
     └─ thread widget: widget.Widget (cửa sổ Win32 layered, tự vẽ bằng Pillow) + cửa sổ thẻ hover
```

Điểm mấu chốt:

1. **Các tiến trình nói chuyện qua FILE**, không qua socket/IPC. Dashboard và hook chỉ đọc `usage_cache.json` + `config.json`;
   muốn "ra lệnh" cho tiến trình nền thì ghi file cờ (`refresh.flag`, `show.flag`, `notify_test.flag`). Vòng loop kiểm tra mỗi giây.
2. **Singleton** bằng `daemon.pid` + `core.daemon_running()`. Chạy exe lần 2 chỉ ghi `show.flag` rồi thoát (để tiến trình nền mở dashboard).
3. **Dashboard là tiến trình riêng** vì WebView2 tốn ~450MB; đóng dashboard là trả hết RAM. Nền chỉ ~63MB. Nút ✕/Esc: ẩn cửa sổ ngay, lưu cỡ, rồi `dashboard.quit_now()` (`TerminateProcess`).
4. **Widget không dùng WebView**: cửa sổ Win32 `WS_EX_LAYERED` per-pixel-alpha, ảnh vẽ bằng Pillow (supersample x3), đẩy lên bằng
   `UpdateLayeredWindow`. Có 2 cửa sổ: `ClaudeUsageWidget` (chính) và `ClaudeUsageTip` (thẻ hover, `WS_EX_TRANSPARENT` nên không chặn chuột).
5. **Hook** chạy `claude-usage.exe hook` mỗi prompt (~0.23s; bản onefile cũ ~0.84s). Nó chỉ đọc cache; **lỗi/không chắc → cho qua**.

### Luồng dữ liệu

`app.loop()` mỗi giây: đọc config nếu đổi → xử lý `show.flag` / `notify_test.flag` / `refresh.flag` → **tối đa 1 request mỗi tick**
cho profile nào đến hạn (`next_at`) → `poll_one()` → `core.refresh_if_expired()` (nếu `token_refresh`; chỉ khi token đã hết hạn) → `core.fetch()` (GET `https://api.anthropic.com/api/oauth/usage`, header
`anthropic-beta: oauth-2025-04-20`, Bearer = accessToken đọc từ `<profile>/.credentials.json`) → cập nhật `self.data` → `write_cache()`
(atomic, có retry) → `refresh_icon()` → `alerts()` → `flush_toasts()`.

Widget (`Widget.tick`, timer 1s; timer nhanh 110ms khi đang fetch hoặc đang hover): đọc cache + config → `build()` → `plan_layout()` (co cỡ/cắt "+N")
→ `render()` → `compose()` (overlay chỉnh sửa + xoay) → `present()`.

## 4. Bản đồ file

| File | Vai trò |
|---|---|
| `core.py` | Hằng số & đường dẫn, `DEFAULTS` (mọi config key), `load/save_config`, `refresh_if_expired()` (làm mới OAuth token + khoá chung với CLI), `fetch()` (+`parse_extra`, `parse_scoped`), `write_cache`/`load_cache`, `effective_interval`, `alias`/`aliases`, `fmt_reset`, `set_autostart`, `launch_cmd`, `log` (→ `events.log`). Không phụ thuộc GUI. |
| `app.py` | Tiến trình nền: poller + backoff 429, tray icon, thông báo (dedupe, baseline lần chạy đầu, batch, rate limit), mở/đưa dashboard lên trước. |
| `widget.py` | Toàn bộ widget: 5 kiểu (`render` = vòng tròn, `render_cards` = bars/cards/segments/arc), `plan_layout` (nhiều profile), `compose` (overlay + xoay), `render_tip`/`render_more_tip` (thẻ hover), class `Widget` (cửa sổ Win32, chuột, kéo/resize/xoay, tip). File lớn nhất (~940 dòng). |
| `dashboard.py` | Tiến trình dashboard (pywebview) + class `Api` (js_api): state, save, hook, và **resize/maximize cửa sổ frameless bằng Win32** (`rect`, `set_rect`, `toggle_max`, `auto_fit`), nhớ cỡ cửa sổ (`dash_rect`). |
| `ui.html` | Giao diện dashboard (HTML/CSS/JS thuần, 1 file). Hai `<script>`: `window.__INIT__` (dữ liệu nhúng sẵn) và logic. |
| `promptguard.py` | Hook CLI. Giữ tên này: tên `hook.py` từng gây rối đóng gói. Ghi lỗi qua `WriteFile` vào stderr handle vì exe là windowed (không có `sys.stderr`). |
| `hookinstall.py` | Cài/gỡ/sửa hook trong `settings.json` của từng profile (idempotent, sao lưu `.claude-usage.bak`, không đụng cấu hình khác, không ghi đè JSON hỏng). |
| `main.py` | Điều phối tham số (xem mục 3). |
| `build.ps1` | Test → `tools/make_assets.py` (icon + version info) → PyInstaller → zip + SHA-256 vào `release\`. |
| `tools/` | `make_assets.py` (bắt buộc cho build) + các script `preview_*.py` render ảnh thiết kế để xem bằng mắt. |
| `tests/` | Xem mục 7. |
| `README.md`, `CHANGELOG.md` | Dành cho người dùng cuối (có trong zip). Cập nhật cùng mỗi tính năng. |

## 5. Dữ liệu & cấu hình

Thư mục `%LOCALAPPDATA%\claude-usage\`:

| File | Nội dung |
|---|---|
| `config.json` | Cấu hình người dùng. `core.load_config()` = `DEFAULTS` + file + tự phát hiện profile nếu danh sách trống + ép `interval_sec >= 90`. File chỉ chứa key đã được lưu; key thiếu lấy từ `DEFAULTS`. |
| `usage_cache.json` | Kết quả poll cho widget/dashboard/hook. Chỉ có số, **không có token**. Ghi atomic (`os.replace` có retry vì Windows từ chối ghi đè file đang được đọc). |
| `alerts.json` | Đã báo cảnh báo nào rồi (`"<account>\|<5h/7d>\|<warn/block>" → giờ reset`). Có file này nghĩa là "không phải lần chạy đầu". |
| `events.log` | Nhật ký toast đã gửi + lỗi API. Tự cắt khi >200KB. Dùng để debug "sao không thấy/sao bị lặp toast". |
| `daemon.pid`, `refresh.flag`, `show.flag`, `notify_test.flag` | Singleton + cờ ra lệnh (xóa sau khi xử lý). |
| `webview\<pid>\` | Dữ liệu WebView2 của từng lần mở dashboard (`dashboard.storage_dir`). Lần mở sau tự xoá thư mục cũ. |

### Schema cache

```jsonc
{ "ts": 1794..., "block_enabled": true, "block_pct": 95, "warn_pct": 80,
  "busy": ["claude-da"],                       // đang gọi API (hoặc xếp hàng do refresh thủ công)
  "refresh": {"ts":..., "started":[...], "skipped":[{"name":..., "why":...}]},   // kết quả lần bấm refresh gần nhất
  "profiles": { "<tên profile>": {
      "path": "...", "five": 23.0, "five_reset": "ISO", "week": 49.0, "week_reset": "ISO",
      "fetched_at": 1794..., "plan": "team",
      "extra":  {"used": 0.98, "limit": 25.0, "pct": 3.92, "currency": "USD"},   // credit dùng thêm, hoặc null
      "scoped": [{"name": "Fable", "pct": 21, "resets_at": "ISO"}],              // giới hạn riêng theo model
      "err": "HTTP 429", "status": 429, "retry_at": 1794...                      // chỉ có khi lần poll gần nhất lỗi (số cũ vẫn giữ)
  } } }
```

Quy ước: profile **có `five`** = có số liệu (có thể đang cũ nếu có `err`); **không có `five`** = chưa từng lấy được.

### Config key (mặc định trong `core.DEFAULTS`)

| Nhóm | Key |
|---|---|
| Poll | `interval_sec` (180, sàn 90; `effective_interval()` còn tăng theo số profile: ≥ 5s/profile) |
| Ngưỡng | `warn_pct` 80, `block_pct` 95, `block_enabled` (hook có chặn không) |
| Thông báo | `notify_enabled`, `notify_warn`, `notify_block` |
| Hệ thống | `token_refresh` (True: tự làm mới token đã hết hạn và ghi `.credentials.json`), `autostart` (ghi `HKCU\...\Run`), `profiles` (rỗng = tự phát hiện `~\.claude` + `~\claude-profiles\*`) |
| Widget: chung | `widget_enabled`, `widget_style` (`rings/bars/cards/segments/arc`), `widget_size` (24-64), `widget_layout` (`auto/vertical/horizontal`), `widget_locked`, `widget_cx/cy` (tâm, null = tự neo cạnh khay), `widget_angle`, `widget_anchor` (`tray/left`), `widget_offset_x/y` |
| Widget: hiển thị | `widget_show_label/5h/7d/reset5/reset7`, `widget_reset` (công tắc tổng cho giờ reset, giữ để tương thích), `widget_tooltip` |
| Widget: giao diện | `aliases` {tên: nhãn}, `widget_label` (`alias/short`), `widget_font` (80-150), `widget_opacity` (30-100), `widget_theme` (`auto/dark/light`), `widget_plate`, `widget_hidden` [tên] |
| Widget: nhiều profile | `widget_sort` (`config/usage`), `widget_max_cells` (0 = tự), `widget_max_len` (px, 0 = tự) |
| Dashboard | `dash_rect` [x,y,w,h] kích thước người dùng đã chọn (null = tự vừa nội dung) |

## 6. Các hành vi/bất biến PHẢI giữ

**Poll & 429** (`app.poll_one`, `core.fetch`)
- Không bao giờ poll nhanh hơn `MIN_INTERVAL=90s`/profile; mỗi profile có lịch `next_at` riêng; **tối đa 1 request/tick (1s)** để giãn.
- Lỗi → **giữ giá trị cũ**, thêm `err/status/retry_at`. 429: chờ `Retry-After` hoặc 120s·2ⁿ (tối đa 900s). 401/credentials: chờ ≥300s. Có jitter.
- Khởi động lại **không bắn API ồ ạt**: `seed()` đọc cache, chỉ fetch khi đã đến hạn. Không có cache → xếp lịch lệch nhau 4s.
- Refresh thủ công: bỏ qua profile vừa fetch <20s hoặc đang trong cửa sổ 429; ghi `refresh.started/skipped` để UI báo.
- Endpoint `/api/oauth/usage` **không chính thức**, không tốn token. Hạn mức gọi tính chung theo account: các tool khác cùng gọi sẽ cộng dồn.

**Làm mới token** (`core.refresh_if_expired`, đã chạy thật 1 lần trên profile thật ngày 2026-11-12, server **có xoay vòng** refresh token)
- Chỉ làm khi `expiresAt` **đã qua** (lúc đó CLI không dùng token này). Cùng `TOKEN_URL`, `CLIENT_ID`, body (`grant_type`, `refresh_token`, `client_id`, `scope`) như CLI;
  `invalid_scope` → thử lại 1 lần không có `scope` (CLI cũng vậy).
- Khoá giống CLI (proper-lockfile, tạo thư mục): `<profile>\.oauth_refresh.lock` **và** `<realpath profile>.lock`. Đang bị giữ → trả `retry_after=30`, không đụng;
  khoá cũ hơn `LOCK_STALE` (90s; CLI cập nhật mtime mỗi 5s, coi >60s là chết) → chiếm. Trong khoá **đọc lại file**: nếu CLI vừa làm mới thì thôi.
- Sao lưu `.credentials.json.claude-usage.bak` **trước** khi POST (sau khi server đã xoay vòng thì không được để lỗi nào chặn việc ghi).
  Ghi atomic, JSON gọn như CLI, giữ mọi key/trường khác. `refreshTokenExpiresAt` = min(cũ, mới) như CLI (không bao giờ kéo dài).
- `invalid_grant` hoặc `refreshTokenExpiresAt` đã qua → `DEAD_MSG` (cần `/login`); nhớ sha256 của token bị từ chối **trong RAM** để không gửi lại tới khi file đổi.
- Token không bao giờ vào log/cache/UI; log chỉ ghi `token refreshed: <profile>`. Lỗi chỉ lấy mã OAuth ngắn (`[a-z_]`), không lấy nội dung body.
- Thông số lấy từ chính `claude.exe` (chuỗi trong binary). Nếu CLI đổi endpoint/client id/cách khoá: grep `TOKEN_URL`, `oauth_refresh.lock` trong binary mới.

**Thông báo** (`app.alerts`, `flush_toasts`)
- Chống trùng theo `(account, 5h/7d, warn/block)` và **so giờ reset có dung sai 30 phút** (API dao động ~1s giữa các lần gọi; từng là nguyên nhân "noti hoài").
- Trạng thái lưu `alerts.json` nên khởi động lại không báo lại. **Lần chạy đầu (chưa có `alerts.json`) chỉ ghi nhận, không báo** (baseline) cho tới khi mọi account đã có dữ liệu hoặc qua 2 phút.
- Luôn ghi trạng thái kể cả khi tắt thông báo (bật lại không "xả" tất cả). Chỉ cảnh báo trên dữ liệu **mới** (không có `err`).
- Gộp: chờ 6s để gom, ≥15s giữa 2 toast, liệt kê ≤4 account + "+N account khác", ≤250 ký tự.

**Hook** (`promptguard.py`)
- Exit 2 + stderr (UTF-8) = chặn. Mọi trường hợp không chắc (cache thiếu/hỏng/quá 15 phút, `block_enabled` tắt, profile không có trong cache) → exit 0.
- Nhận diện profile bằng `CLAUDE_CONFIG_DIR` (mặc định `~\.claude`). `hookinstall` **không bao giờ tự chạy**; chỉ khi người dùng bấm nút trong dashboard.

**Widget**
- Không vượt màn hình: `plan_layout` co cỡ (sàn 60% hoặc 20) rồi cắt, ô cuối là `+N khác` (`MORE="__more__"`). Ô "+N" tính vào số chỗ cần.
- Thứ tự dữ liệu thống nhất: `visible_profiles()` (lọc `widget_hidden`, sắp xếp) → `apply_plan()` → `self.view` là **nguồn duy nhất cho hover/tip**.
- Mọi thứ đổi được qua config phải nằm trong `sig` của `Widget.tick` (nếu không widget không vẽ lại khi đổi).
- Taskbar có thể ở **cạnh phải, dọc** (máy của chủ dự án): `taskbar()` + `auto_center()` hỗ trợ ngang/dọc; chuyển layout theo hướng.
- Tự ẩn khi toàn màn hình (`SHQueryUserNotificationState`) hoặc taskbar không hiện.

**Dashboard**
- Cửa sổ frameless nên **tự làm resize**: 8 div cạnh/góc trong `ui.html` → `pointer` events → `api.rect()/set_rect()` (SetWindowPos, pixel vật lý, process đặt DPI-aware).
  `user_sized` = người dùng đã chọn cỡ → `fit()` (tự vừa nội dung) bị tắt, chỉ bật chế độ `compact` nếu nội dung cao hơn cửa sổ.
- Không thanh cuộn thấy được: `main` cuộn nội bộ, thumb trong suốt trừ khi hover. Test e2e kiểm tra "không cần cuộn" ở nhiều chiều cao màn hình giả lập.

## 7. Kiểm thử

Mọi test dùng thư mục dữ liệu sandbox. **Chỉ chạy MỘT test GUI tại một thời điểm** (cạnh tranh cửa sổ/timer sẽ gây flaky).

| Lệnh | Cần | Thời gian | Kiểm tra gì |
|---|---|---|---|
| `tests\test_unit.py` | không | ~40s | 67 test: parse API, làm mới token (server giả, khoá, xoay vòng, lỗi), backoff, busy, refresh, toàn bộ logic thông báo, hook (chạy bằng subprocess), cài/gỡ hook, render widget (mọi kiểu × bố cục × tổ hợp tick), hover card, nhiều profile, nhãn, JS parse |
| `tests\e2e_widget.py` | màn hình | ~2 phút | Widget thật: gửi message chuột (kéo/resize/xoay/khoá), spinner, kiểu, hover card, nhiều account, ô +N |
| `tests\e2e_dashboard.py x\|esc` | màn hình | ~1 phút | Dashboard thật bằng JS click: mọi nút, tab, tự áp dụng, không cuộn, màn 768px, hook (profile sandbox) |
| `tests\e2e_responsive.py` | màn hình | ~1.5 phút | 14 profile, kéo cạnh, thu nhỏ hết cỡ, phóng to/khôi phục, nhớ cỡ qua 2 lần mở |
| `tests\e2e_app.py` | màn hình + **gọi API thật** | ~6 phút | App nền thật: thông báo bật/tắt/lặp/khởi động lại/lần đầu, refresh, trạng thái busy. Đọc token thật (chỉ đọc). |
| `tests\smoke_release.py` | màn hình + API | ~1 phút | Giải nén chính file zip, chạy exe đóng gói: khởi động, widget, dashboard, 1 instance, hook, credit parse thật |

Chạy từ PowerShell 7 nên dùng `Start-Process ... -RedirectStandardOutput` + `WaitForExit(timeout)` để không treo (xem mục 9).
Quy ước viết test mới: `@test` + `check(name, ok, detail)`; test unit phải sandbox; GUI e2e thoát bằng `os._exit(0|1)`.

## 8. Quy trình phát hành

1. Sửa code + test. Cập nhật `README.md`, `CHANGELOG.md`; nếu đổi phiên bản: `core.__version__` (icon/version info tự lấy từ đó).
2. `powershell -ExecutionPolicy Bypass -File build.ps1` (tự chạy unit test; fail là dừng).
3. Chạy `tests\smoke_release.py` (kiểm tra **gói**, không phải source). Chạy thêm `e2e_*` nếu đụng vùng liên quan.
4. Mở lại app thật: `Start-Process .\dist\claude-usage.exe -ArgumentList "--hidden"`.
5. Ghi SHA-256 mới (in ra cuối `build.ps1`, cũng nằm trong `release\*.sha256`).
- **Chưa đăng đi đâu cả.** Đăng GitHub/chỗ khác chỉ khi người dùng yêu cầu rõ và nói nơi đăng.
- Exe chưa ký số → SmartScreen có thể cảnh báo (README đã nói).

## 9. Bẫy đã dính (đọc kỹ, tiết kiệm nhiều giờ)

1. **Heredoc / tham số lệnh làm hỏng `\n`.** Khi viết code Python/JS qua `python - <<EOF`, `sed`, hoặc tham số tool, chuỗi `"\n"` hay bị biến thành
   **xuống dòng thật** → `SyntaxError: unterminated string literal` hoặc file hỏng âm thầm. Đã dính >8 lần. Dùng công cụ Edit/Write cho các dòng có `\n`,
   hoặc `chr(10)`. Sau mỗi lần sửa: `python -W error -m py_compile <file>` và (cho `ui.html`) kiểm tra cú pháp JS bằng `node` (unit test đã có).
2. **pywebview duyệt đệ quy mọi thuộc tính công khai của object `js_api`.** Gắn `Window` vào thuộc tính công khai → đệ quy vô hạn, dashboard load rất chậm/treo.
   Luôn dùng tên **riêng tư** `_window` (đã có comment trong `dashboard.Api`).
3. **`ImageDraw` trên ảnh RGBA GHI ĐÈ pixel, không trộn alpha.** Muốn lớp phủ bán trong suốt phải vẽ ra layer riêng rồi `alpha_composite`.
   Cũng vì vậy marker "đang fetch" phải vẽ **sau** nền thẻ (từng bị che mất ở kiểu `cards`).
4. **PyInstaller bỏ qua module có lỗi cú pháp** và chỉ báo "invalid module" trong `build\...\warn-*.txt`; exe chạy sẽ báo `ModuleNotFoundError`.
   Khi gặp lỗi kiểu này, trước hết `py_compile` module đó. Các import động/lazy cần `--hidden-import` (đã liệt kê trong `build.ps1`).
5. **Exe windowed không có `sys.stderr`** → hook ghi lỗi bằng `GetStdHandle`/`WriteFile`. Từ 2026-11-19 build onedir (không còn giải nén mỗi lần chạy); số đo ở `bench/BASELINE.md`.
6. **Windows từ chối `os.replace` lên file đang được đọc** → `write_cache` thử lại; `load_cache` cũng thử lại. Test tải 250 lần ghi/giây đã có.
7. **`Get-FileHash` có thể không nạp được** khi PowerShell 5.1 được khởi từ PowerShell 7 → `build.ps1` dùng .NET SHA256. PyInstaller log ra stderr nên
   `$ErrorActionPreference="Stop"` coi là lỗi → đã bọc bằng `Continue` + kiểm tra exit code.
8. **Encoding console cp1252**: in tiếng Việt/ký tự `█` ra pipe có thể ném `UnicodeEncodeError` và làm chết bộ test. Bộ test tự `reconfigure(utf-8)`.
   Đọc `.out` file của tiến trình con bằng cách chờ `WaitForExit` (đọc pipe có thể kẹt vì tiến trình con WebView2 giữ pipe vài giây).
9. **Test chuột giả**: `PostMessage` mouse-move bị Windows huỷ bằng `WM_MOUSELEAVE` (con trỏ thật ở nơi khác) → test stub `TrackMouseEvent` và dùng
   `SendMessageW` (đồng bộ, chạy đúng trên luồng widget). Không gọi `_wndproc` từ luồng test (race với luồng widget).
10. **Màn hình khoá** → Windows thay nội dung toast bằng "Private" (không phải lỗi app). Toast chỉ kiểm chứng bằng mắt khi máy mở khoá; luôn có `events.log`.
11. **KHÔNG chụp màn hình để "xem thử" cửa sổ** nếu cửa sổ có thể nằm sau cửa sổ khác: ảnh sẽ chứa nội dung riêng tư của người dùng (từng chụp trúng trình duyệt).
    Nếu bắt buộc chụp: đặt cửa sổ `on_top=True` và chỉ cắt đúng vùng của nó; không mô tả/lưu nội dung không liên quan.
12. **Process ngoài của người dùng**: máy có một tool khác `claude-usage-widget.exe` (trong Downloads) cũng gọi cùng endpoint → cộng dồn hạn mức gọi.
    Khi dọn tiến trình chỉ kill theo `Name='claude-usage.exe'`, đừng kill theo pattern rộng.
13. `render()` mỗi frame spinner tốn thời gian ∝ số profile; `plan_layout` có cache theo "khoá cấu trúc" (`Widget.plan_key`) để không đo lại mỗi frame. Đừng bỏ cache này.
14. `widget_reset` (công tắc tổng) và `widget_show_reset5/7` cùng tồn tại để tương thích; hiệu lực = `widget_reset AND widget_show_resetN`. Dashboard giữ chúng đồng bộ.
15. **Phần tử ẩn bằng `opacity:0` vẫn bắt chuột.** `#toast` (góc phải dưới) từng đè lên nút `refresh`/`save` sau lần toast đầu → "bấm không ăn". Lớp phủ/thông báo phải có `pointer-events:none`; `e2e_dashboard` kiểm tra mọi nút bằng `elementFromPoint`.
16. **Thoát dashboard**: `window.destroy()` + `os._exit` mất ~2s vì pywebview dispose WebView2 rồi chờ tới 3s để xoá thư mục dữ liệu tạm.
    Giờ dùng `Api.hide()` → `ShowWindowAsync(SW_HIDE)` → `shutdown()` (huỷ timer lưu cỡ, giữ `CFG_LOCK`, lưu cỡ) → `quit_now()` (`TerminateProcess`, ~20–40ms).
    Vì bỏ qua bước dọn của pywebview nên thư mục dữ liệu WebView2 nằm ở `DATA\webview\<pid>` và được tự dọn ở lần mở sau.
    Alt+F4 đi cùng đường qua `events.closing`. Gọi `TerminateProcess` qua ctypes **bắt buộc khai báo `argtypes`**: pseudo-handle `-1` thành số
    64-bit không vừa `int`, gây lỗi `OverflowError` âm thầm rồi rơi về `os._exit` chậm. `App.show()` chạy trên luồng riêng và chờ tối đa 2.5s
    khi tiến trình dashboard còn sống mà không có cửa sổ (bản onefile: bootloader còn đang dọn `_MEI`). Đo thời gian thoát qua handle tiến trình,
    đừng đo qua pipe stdout (bẫy 8).
17. **Gọi `pywebview.api` trước sự kiện `pywebviewready`** sẽ ném lỗi và mất cú bấm (trang hiện ngay nhờ dữ liệu nhúng sẵn). Trong `ui.html` luôn gọi qua
    proxy `api.X(...)`: proxy chờ cầu nối sẵn sàng. Mọi thao tác load-modify-save `config.json` trong `dashboard.Api` phải nằm trong `CFG_LOCK`;
    `core.save_config` ghi kiểu nguyên tử.
18. **Chớp / nhảy trên dashboard**: (a) ghi `innerHTML` mỗi lần poll làm chạy lại animation (spinner, pulse) → `apply()` chỉ dựng lại `#accs` khi chuỗi HTML
    đổi; chữ thay đổi theo thời gian (`ago/until/reset`, tooltip) nằm trong `<span class="v">` và được `fillLive()` điền tại chỗ; footer dùng `setHTML()`.
    (b) `fit(shrink)`: thay đổi nền chỉ được làm cửa sổ cao thêm, chỉ co khi người dùng thao tác (tab, tự vừa, mở). (c) Cửa sổ tạo `hidden=True`,
    hiện bằng `window.show()` sau khi trang gọi `api.page_ready()` (đã poll + fit). `window.resize()` của pywebview có cờ `SWP_SHOWWINDOW` nên
    `Api.fit` tự gọi `SetWindowPos`. Trang ẩn bị bóp hẹn giờ (~1s) nên đừng dùng `setTimeout` trong chuỗi khởi động. `.title`/`footer` phải `flex:none`
    (từng bị co 34→27px khi cửa sổ thấp, làm phép đo lệch 7px). Mọi thao tác hình học cửa sổ trong `Api` giữ `GEO_LOCK`.
19. **KHÔNG đặt `argtypes`/`restype` lên `ctypes.windll.user32/kernel32` trong tiến trình dashboard.** Đối tượng đó dùng chung với pywebview:
    `window.move` của pywebview (kéo thanh tiêu đề) truyền `None` cho `SetWindowPos`, và argtypes của mình làm nó ném `TypeError` âm thầm → không kéo
    được cửa sổ. `dashboard.py` dùng bản riêng `ctypes.WinDLL("user32")`. `e2e_dashboard` có check kéo cửa sổ.

## 10. Quy tắc & ràng buộc khi làm việc

- **Chính sách dữ liệu của tổ chức (Upheads)**: không đưa vào code/tài liệu/log/commit/ảnh chụp: credential, token, API key, thông tin cá nhân của khách hàng/nhân viên,
  cấu hình bảo mật nội bộ. App **đọc** token OAuth của các profile Claude Code ở máy người dùng và chỉ **ghi** lại `.credentials.json` khi tự làm mới
  token đã hết hạn (`core.refresh_if_expired`, chủ dự án đồng ý ngày 2026-11-12). Không bao giờ in/log/gửi token đi đâu ngoài endpoint của Anthropic.
  Test chỉ dùng token giả (`"test-token-not-real"`) hoặc đọc token thật trong e2e (không in ra). Dữ liệu mẫu trong test là số giả.
- **Hỏi trước khi làm điều khó đảo ngược/hướng ra ngoài**: cài hook vào `settings.json` thật của profile, đăng release ra ngoài, xoá/ghi đè dữ liệu người dùng.
- Không sửa cấu hình của người dùng ngoài phạm vi (từng làm bẩn `config.json` thật bằng test, nhớ **sao lưu/khôi phục**; ưu tiên sandbox `LOCALAPPDATA`).
- Báo cáo trung thực: nói rõ cái gì đã test thật, cái gì chỉ test giả lập, cái gì chưa kiểm chứng.
- Giao tiếp bằng tiếng Việt, ngắn gọn; giải thích nguyên nhân, không chỉ triệu chứng.

## 11. Nhật ký quyết định (vì sao như vậy)

- **Tự làm mới token (1.1.0)**: profile ít dùng hết hạn token (~8h) nên kẹt `token hết hạn`. Chủ dự án chọn để app tự refresh và ghi lại file như CLI
  (thay vì chỉ đọc). Không thể refresh mà chỉ giữ token trong RAM: server xoay vòng refresh token, file sẽ giữ token chết và CLI phải `/login` lại.
- **Chuyển sang Rust (2026-11-19)**: trước đây giữ Python vì việc chính là gọi HTTP vài phút một lần. Người dùng thấy lag nên quyết định port toàn bộ
  + refactor dashboard (Tauri v2 + Svelte). Đo mốc ở `bench/BASELINE.md`: lag lớn nhất là hook onefile (~0.84s/prompt) và mở dashboard (~1.5s);
  vẽ widget với 4 profile chỉ 8–12ms/frame. Trong lúc port đã đổi build sang onedir (hook ~0.23s). Kế hoạch + review + các quyết định: `PORT_PLAN.md`.
- **Widget bằng layered window + Pillow, không WebView**: để nền trong suốt thật, antialias đẹp, và RAM nền nhỏ. Dashboard WebView tách tiến trình để trả RAM khi đóng.
- **Poll theo lịch từng profile + giữ số cũ khi lỗi**: bản đầu ghi đè số bằng lỗi nên widget trống khi 429.
- **Giao tiếp qua file**: đơn giản, debug được bằng mắt, không cần cổng/quyền; chấp nhận độ trễ ≤1s.
- **Frameless dashboard + tự làm resize**: giữ giao diện terminal; pywebview không hỗ trợ resize cho frameless.
- **Widget luôn tự neo "tay trái tray"** (hoặc phía trên khay nếu taskbar dọc) vì không đọc được chỗ trống giữa icon taskbar (XAML); giới hạn `stack_limit` = 50% chiều cao màn hình khi gắn taskbar.
- **Mặc định kiểu `rings`**; chủ dự án tự đổi kiểu trong config thật (lần gần nhất là `cards`) và chưa chốt kiểu mặc định cho bản phát hành: hỏi trước khi đổi `DEFAULTS["widget_style"]`.

## 12. Việc còn lại / ý tưởng (chưa làm)

Chưa kiểm chứng (ưu tiên cao):
- Click/kéo bằng chuột thật; cài hook thật rồi thử `claude` CLI bị chặn thật khi hạ `block_pct` tạm thời.
- Windows 10, máy không có WebView2 Runtime (hiện không có màn hình hướng dẫn cài), nhiều màn hình (widget neo màn hình chính), DPI ≠ 100% (đã tính `GetDpiForSystem`, test mới ở 100%).

Ý tưởng (hỏi chủ dự án trước khi làm):
- Tuỳ chỉnh màu xanh/cam/đỏ; thứ tự account kéo thả; ngưỡng riêng từng account; chỉ hiện 5h hoặc 7d theo account.
- Ký số exe, installer (MSIX/Inno), tự cập nhật; đổi hook sang bản nhanh hơn.
- Tự phát hiện profile mới khi người dùng thêm (hiện chỉ lúc cấu hình rỗng).
- Hiển thị lịch sử dùng.

## 13. Checklist khi thêm một tính năng/config

1. Thêm key vào `core.DEFAULTS` (và clamp trong `load_config` nếu cần).
2. Nếu ảnh hưởng widget: thêm vào `sig` trong `Widget.tick`, và vào khoá `plan_key` trong `Widget.build` nếu đổi kích thước.
3. Nếu có UI: sửa `ui.html` (load trong `apply()`, lưu trong `save()`, tự áp dụng nếu nằm trong tab giao diện), kiểm tra cú pháp JS.
4. Viết test unit (logic/vẽ) **và** e2e (nếu có tương tác); chạy `test_unit.py`, rồi e2e liên quan, rồi `build.ps1` + `smoke_release.py`.
5. Cập nhật `README.md`, `CHANGELOG.md`, và file này (mục 5, 6, 9 nếu có bẫy mới). Cập nhật memory/ghi chú nếu agent có cơ chế đó.
