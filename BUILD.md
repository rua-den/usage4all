# Build Rust 2.0.0

Bản Python (cũ) vẫn build bằng `build.ps1`. Bản Rust nằm ở `rust/`, build bằng `build-rust.ps1`. Giữ bản Python để rollback; không cần dời hoặc xoá bản đang cài.

## Cần có (một lần)
- Rust toolchain qua `rustup` (phiên bản ghim trong `rust/rust-toolchain.toml`, tự tải).
- Visual Studio Build Tools (C++ / MSVC linker) + Windows 10/11 SDK. SDK cung cấp `rc.exe` để nhúng icon và version vào exe; thiếu thì build vẫn qua, chỉ có cảnh báo `resource embedding skipped`.
- WebView2 Runtime để chạy dashboard (Windows 11 có sẵn).
- Không cần Python hay Node.

## Build + đóng gói (một lệnh)
```powershell
powershell -ExecutionPolicy Bypass -File build-rust.ps1             # clippy + test + build + zip
powershell -ExecutionPolicy Bypass -File build-rust.ps1 -SkipTests  # chỉ build + zip (nhanh)
```
Kết quả trong `release\`: thư mục `claude-usage-rust-v<ver>-win-x64\` (2 exe + SHA256SUMS.txt + tài liệu), file `.zip` và `.zip.sha256`.
Phiên bản lấy từ `rust/Cargo.toml` (`[workspace.package] version`). Đổi số đó khi phát hành.

Script chỉ tắt `claude-usage*.exe` đang chạy từ `rust\target` hoặc `release\` (bản vừa build). Không đụng `dist\` (bản Python) hay bản đã cài ở chỗ khác. Nếu bạn đang chạy thử bản Rust từ `release\`, nó sẽ bị tắt khi build.

## Chạy từng phần (trong `rust/`)
```powershell
cargo build --locked --release # target\release\claude-usage.exe + claude-usage-dashboard.exe
cargo clippy --locked --workspace --all-targets --release -- -D warnings
cargo test --locked --workspace --release --features "claude-usage-dashboard/e2e,claude-usage/e2e" -- --test-threads=1
cargo test -p cu-core          # chỉ logic thuần + parity với Python (nhanh, không GUI)
```
Quy tắc test:
- Test GUI (`tray_ui`, `widget_ui`, `dashboard_ui`, `tray_opens_dashboard`) chạy **tuần tự** (`--test-threads=1`) và **không có tray Rust nào đang chạy** (test sẽ từ chối). Bản Python đang chạy không ảnh hưởng.
- Mọi test dùng `LOCALAPPDATA`/`USERPROFILE` sandbox, token giả, không gọi API thật, không ghi registry (`CU_NO_AUTOSTART`) hay hook thật (`CU_NO_REPAIR`).
- Feature `e2e` chỉ để test dashboard. **Bản phát hành phải build không có nó** (`build-rust.ps1` đã lo; build tay thì `cargo build --release` không kèm `--features`).
- Test mạng thật (mặc định bị bỏ qua): `cargo test -p claude-usage --release -- --ignored` (gửi request không token tới endpoint, mong 401).

## Smoke test gói đã build
```powershell
.\.venv\Scripts\python tests\smoke_release_rust.py [zip]   # mặc định: zip Rust mới nhất trong release
```
Giải zip vào thư mục tạm, chạy trong sandbox (profile giả không có token, không gọi mạng, không đụng registry/hook thật): 1 instance, tray, widget, dashboard mở/đóng, hook, Quit, sha256, exe dashboard bỏ qua `--e2e`. Từ chối chạy nếu đã có tray/dashboard Rust khác. Chạy sau mỗi lần build-rust.ps1.

Test GUI chi tiết (mỗi cái 40–80 giây, tổng full test ~6 phút): `dashboard_legacy`, `dashboard_responsive` (port e2e_dashboard/e2e_responsive), `widget_e2e` (port e2e_widget), `app_e2e` (port e2e_app, chạy exe thật với server HTTP giả trên 127.0.0.1).

Seam chỉ có khi build debug hoặc `--features e2e` (CU_API_BASE, CU_TOKEN_URL, widget_state.json, fake_taskbar.txt, fake_fullscreen.flag, `e2e_stage`/`e2e_go` của dashboard). `build-rust.ps1` build lại bản sạch và dừng nếu còn chuỗi seam trong exe; smoke test kiểm lại trên exe trong zip.

Test chuột thật (`widget_realmouse`, `dashboard_drag`) dùng SendInput nên cần máy đang MỞ KHOÁ màn hình và không ai chạm chuột lúc chạy; máy khoá thì fail (input desktop không dùng được). Test dpi_scale kiểm bằng toán thuần vì không có màn hình thứ hai.

Muốn chạy full test phải thoát tray Rust đang dùng (các test GUI từ chối khi đã có tray khác).

Chưa phủ so với test Python cũ: API thật với token thật (chỉ kiểm bằng cách quan sát bản đang chạy thật: events.log và usage_cache.json); đổi DPI thật khi kéo sang màn hình khác; toast WinRT hiện thật; xem hình đẹp/xấu.

## Sinh lại fixture parity (cần Python venv của bản cũ)
`tools\gen_fixtures*.py` chạy code Python thật rồi ghi JSON vào `rust/crates/core/tests/fixtures/`. Chỉ cần chạy lại khi đổi hành vi bên Python.

## Xem thử widget không cần chạy app
```powershell
rust\target\release\claude-usage.exe preview <thư-mục-ra>   # xuất PNG các kiểu widget
```

## Cài / chạy
Giải nén zip, **để 2 exe cạnh nhau**, chạy `claude-usage.exe`. Tick "Chạy cùng Windows" trong dashboard để autostart (ghi `HKCU\...\Run`, ghi đè mục của bản Python). Không chạy song song bản Python và bản Rust (cộng dồn lượt gọi API).
Rollback: thoát app Rust, mở lại bản Python.

## Cấu trúc
- `rust/crates/core` (cu-core): logic thuần (config, fmt, scheduler, alerts, hook, widget layout, net/refresh token), có parity test với Python.
- `rust/app` (`claude-usage.exe`): tray, daemon, widget, hook (`claude-usage.exe hook`), autostart.
- `rust/dashboard` (`claude-usage-dashboard.exe`): cửa sổ dashboard (wry + WebView2) dùng `ui.html` ở gốc repo.

Chi tiết kiến trúc và bẫy: `HANDOFF.md`. Kế hoạch và trạng thái: `PORT_PLAN.md`.

## Xác nhận trước khi phát hành

- Workflow Windows chạy kiểm tra biên dịch, fixture PAC loopback qua WinHTTP thật (DIRECT và proxy, không token, không sửa registry), các test tự động và smoke test gói. Hai test cần chuột thật vẫn phải chạy riêng trên desktop không khoá.
- Test PAC không thay thế kiểm tra WPAD hoặc kho chứng chỉ trên mạng doanh nghiệp thật. Kiểm tra các trường hợp đó trước khi mở rộng rollout. Đóng gói không tự tạo tag hay GitHub Release.

## Chưa có
Ký số exe, icon cho toast (cần `icon.png` cạnh exe). Toast WinRT (AUMID `ClaudeUsage.App`, ghi HKCU, chỉ bản release) chưa được xem hiện trên màn hình thật; lỗi thì tự rơi về balloon và ghi `winrt toast failed` vào events.log.
