# claude-usage

Theo dõi **hạn mức Claude (5h / 7d) của nhiều account cùng lúc** trên Windows: icon ở tray, widget nhỏ trên taskbar,
dashboard, thông báo khi gần hết và (tuỳ chọn) chặn prompt của Claude Code CLI khi account đã quá ngưỡng.

Giải nén cả thư mục rồi chạy `claude-usage.exe` (cần thư mục `_internal` nằm cạnh exe). Không cần cài đặt, không cần Python.

## Cách dùng nhanh

1. Chạy `claude-usage.exe`. Lần đầu nó tự tìm các profile Claude Code của bạn (`~\.claude` và `~\claude-profiles\*`).
2. Dashboard hiện ra, một icon xuất hiện ở khay hệ thống, một widget nằm cạnh khay trên taskbar.
3. Đóng dashboard (✕ hoặc `Esc`) thì app vẫn chạy nền ở tray. Muốn mở lại: click icon tray hoặc click widget.
4. Muốn thoát hẳn: chuột phải icon tray (hoặc widget) → **Quit**.

## Tính năng

| | |
|---|---|
| **Tray icon** | Số % cao nhất trong các account, đổi màu xanh / cam / đỏ theo ngưỡng. Chấm xanh dương ở góc = đang gọi API. |
| **Widget taskbar** | 5 kiểu giao diện, đổi trong dashboard hoặc chuột phải widget → *Next style*: **vòng tròn** (ngoài = 5h, trong = 7d), **thanh ngang**, **thẻ số lớn**, **ô vuông kiểu terminal**, **đồng hồ bán nguyệt**. Chọn hiển thị gì bằng các ô tick (tên account, 5h, 7d, giờ reset 5h, giờ reset 7d) để widget gọn lại. **Rê chuột vào một account để xem thẻ chi tiết**: 5h và 7d kèm giờ reset cụ thể, giới hạn riêng theo model (như Fable), credit dùng thêm trong tháng (đã dùng / hạn mức), trạng thái cập nhật. **Tab "Giao diện widget"** (áp dụng ngay, không cần save): tên riêng cho từng account hoặc dùng tên ngắn thay cho viết tắt, cỡ chữ 80–150%, độ đậm 30–100%, màu chữ (theo Windows / sáng / tối), nền mờ phía sau cho dễ đọc, ẩn bớt account khỏi widget. Tên dài tự co chữ rồi cắt bằng … Vòng xanh dương quay quanh account đang được cập nhật. Chấm cam + màu mờ = đang hiển thị dữ liệu cũ (ví dụ đang bị giới hạn 429). |
| **Kéo thả widget** | Mở khoá (dashboard hoặc chuột phải) → kéo thân để di chuyển, ô vuông góc dưới-phải để đổi cỡ, nút tròn phía trên để xoay, ổ khoá để khoá lại. Chuột phải → *Reset* để về vị trí tự động. |
| **Thông báo** | Toast khi một account vượt ngưỡng cảnh báo hoặc ngưỡng chặn. **Mỗi chu kỳ 5h / 7d chỉ báo 1 lần**, gộp nhiều cảnh báo thành 1 toast gọn (mỗi account một dòng + gợi ý account còn nhiều quota nhất), tối thiểu 15s giữa 2 toast, nhớ qua lần khởi động lại. **Lần chạy đầu tiên chỉ ghi nhận những gì đang cao chứ không báo ngay**, chỉ báo khi có thay đổi mới. Bật / tắt tổng, bật / tắt riêng "cảnh báo" và "chặn", có nút gửi thử. |
| **Chặn CLI** | Hook `UserPromptSubmit` của Claude Code: khi account hiện tại ≥ ngưỡng chặn thì prompt bị từ chối và gợi ý đổi sang account còn nhiều quota nhất. Nếu không chắc (cache cũ, lỗi) thì **cho qua**. |
| **Chống 429** | Không bao giờ poll nhanh hơn 90 giây. Lịch riêng cho từng account, bị 429 thì lùi dần (2 → 4 → … tối đa 15 phút, hoặc theo `Retry-After`) và **giữ giá trị cũ** thay vì xoá. Nút refresh bị bỏ qua nếu vừa cập nhật dưới 20 giây hoặc đang trong thời gian chờ 429. |

## Nhiều profile

- **Dashboard** kéo giãn được từ mọi cạnh và góc, có nút phóng to / khôi phục (hoặc nhấp đúp thanh tiêu đề). Bố cục tự đổi theo bề ngang: rộng thì nhiều cột, hẹp thì 1 cột; thấp hơn nội dung thì cuộn bên trong (thanh cuộn chỉ hiện khi rê chuột vào). App nhớ cỡ và vị trí cửa sổ bạn đã chọn; nút **⤢ tự vừa** ở chân cửa sổ để quay về tự vừa nội dung. Cửa sổ mặc định rộng hơn khi có nhiều profile.
- **Widget** không bao giờ lớn quá màn hình: khi có nhiều account nó tự co cỡ (tối thiểu 60%), nếu vẫn không đủ thì hiện các account đầu và một ô **+N khác** (rê chuột vào để xem danh sách). Trong tab *Giao diện widget* có thể chọn **sắp xếp theo mức dùng** (account gần hết hạn mức luôn hiện đầu) và **số account tối đa** (0 = tự vừa màn hình). Nhãn tự sinh luôn khác nhau (`T01`, `T02`…).
- Chu kỳ cập nhật tự giãn ra khi có rất nhiều profile (khoảng 1 request / 5 giây) để không bị giới hạn tần suất. Thông báo chỉ liệt kê 4 account, phần còn lại gộp thành "+N account khác".

## Cài hook chặn CLI (tuỳ chọn)

Dashboard → **cài hook vào các profile**. Nó thêm một mục `UserPromptSubmit` vào `settings.json` của từng profile
(giữ nguyên mọi thứ khác, tạo bản sao lưu `settings.json.claude-usage.bak` lần đầu). **gỡ hook** để xoá đúng mục đó.
Nếu bạn dời thư mục app sang chỗ khác (exe cùng `_internal`), mở app một lần là hook tự được sửa đường dẫn.
Mỗi lần hook chạy mất khoảng 0.25 giây.

## Chạy cùng Windows

Dashboard → tick **Chạy cùng Windows**. App khởi động ẩn ở tray (ghi vào `HKCU\...\Run`, không cần quyền admin).
Nếu dời exe: mở ở chỗ mới một lần, đường dẫn tự cập nhật.

## Dữ liệu & quyền riêng tư

- App đọc token OAuth trong `.credentials.json` của từng profile để gọi `https://api.anthropic.com/api/oauth/usage`
  (đọc số liệu hạn mức, **không chạy model nên không tốn token**). Token **không bao giờ** bị in ra, ghi log hay lưu vào file của app.
- **Tự làm mới token hết hạn** (bật mặc định, tắt được trong Dashboard → tab Chung): profile nào lâu không dùng `claude` thì token
  hết hạn sau khoảng 8 giờ. Khi đó app làm đúng như `claude` CLI: gửi refresh token tới `https://platform.claude.com/v1/oauth/token`,
  rồi ghi token mới vào `.credentials.json` (giữ nguyên mọi trường khác, dùng chung file khoá với CLI để hai bên không giẫm nhau,
  bản trước đó được lưu ở `.credentials.json.claude-usage.bak`). App chỉ làm việc này khi token **đã** hết hạn.
- Không gửi gì tới nơi nào khác, không telemetry.
- Dữ liệu của app nằm ở `%LOCALAPPDATA%\claude-usage\`: `config.json`, `usage_cache.json` (chỉ có số %, không có token),
  `alerts.json` (đã báo gì rồi), `events.log` (nhật ký toast / lỗi API, tự cắt gọn), `webview\` (dữ liệu tạm của cửa sổ dashboard, tự dọn).

## Xử lý sự cố

- **Dòng báo `HTTP 429`**: bị giới hạn tần suất gọi API. App tự chờ rồi thử lại; số liệu cũ vẫn hiện (mờ, kèm chấm cam). Đừng chạy song song nhiều tool khác cùng gọi endpoint này cho cùng account, hạn mức tính chung.
- **`token hết hạn`**: chỉ gặp khi đã tắt *Tự làm mới token hết hạn*. Mở `claude` ở profile đó một lần để nó tự làm mới token.
- **`phiên đăng nhập hết hạn - chạy claude ở profile này rồi /login`**: server từ chối refresh token (đăng nhập quá lâu hoặc đã đăng xuất).
  Mở `claude` ở profile đó và `/login` lại; app tự nhận token mới.
- **Không thấy toast / toast chỉ ghi "Private"**: Windows ẩn nội dung thông báo khi máy đang khoá màn hình, và đưa thẳng vào Action Center khi bật *Focus assist / Không làm phiền*. Mở khoá rồi bấm **gửi thử thông báo** để kiểm tra. `%LOCALAPPDATA%\claude-usage\events.log` ghi mỗi toast đã gửi.
- **Widget không hiện**: có thể taskbar đang tự ẩn, hoặc bạn đang ở chế độ toàn màn hình (widget tự ẩn). Tray → *Show widget*.
- **SmartScreen cảnh báo lần đầu**: file chưa được ký số. Chọn *More info → Run anyway*. Kiểm tra SHA-256 trong file `.sha256` đi kèm.

## Gỡ cài đặt

Quit app → xoá thư mục chứa `claude-usage.exe` và `_internal` → xoá thư mục `%LOCALAPPDATA%\claude-usage\` → (nếu đã cài hook) dùng *gỡ hook* trước khi xoá app,
và bỏ tick *Chạy cùng Windows*.

## Giới hạn đã biết

- Endpoint usage **không có tài liệu chính thức**; Anthropic có thể đổi, khi đó app cần cập nhật.
- Chỉ Windows 10/11 (cần WebView2 runtime, có sẵn trên Windows 11).
- Widget hỗ trợ taskbar ở cạnh dưới / trên / phải / trái của màn hình chính; trên nhiều màn hình widget chỉ neo ở màn hình chính.
- Exe chưa được ký số.

## Build từ source

```powershell
python -m venv .venv; .\.venv\Scripts\pip install pystray pillow pywebview pyinstaller
powershell -ExecutionPolicy Bypass -File build.ps1        # test + build + đóng gói vào release\
```
Test: `.venv\Scripts\python tests\test_unit.py` (nhanh, không cần mạng) · `tests\e2e_app.py`, `tests\e2e_widget.py`, `tests\e2e_dashboard.py` (cần màn hình, gọi API thật vài lần).
