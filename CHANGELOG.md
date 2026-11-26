# Changelog

## 2.0.0 (chờ xác nhận phát hành)

- Rust là bản build chính: `claude-usage.exe` (tray, daemon, widget và hook) và `claude-usage-dashboard.exe` (WebView2). Gói gồm hai exe đặt cạnh nhau, không có `_internal`, có SHA-256.
- Config, cache, thông báo và hook tương thích với bản Python 1.1.0. Bản Python vẫn có thể build bằng `build.ps1` để quay lại; không chạy đồng thời hai bản.
- Proxy: biến môi trường hợp lệ ưu tiên; PAC/WPAD theo từng URL qua WinHTTP, sau đó đến proxy tĩnh và ProxyOverride nếu không phân giải được. Không tự đăng nhập với máy chủ PAC. TLS sử dụng kho chứng chỉ Windows.
- Toast WinRT với AUMID riêng, fallback balloon; dashboard chạy trong Job Object của tray. Có test parity Python, test tiến trình tray/widget/dashboard và fixture PAC Windows không chứa thông tin đăng nhập.
- Windows CI kiểm tra build/test/gói; chưa coi CI là kiểm chứng WPAD doanh nghiệp, chuột thật hay chữ ký số.

## 1.1.0

- Mới: **tự làm mới token hết hạn**. Trước đây profile nào lâu không mở `claude` (token sống khoảng 8 giờ) sẽ kẹt ở `token hết hạn` cho tới khi bạn chạy `claude` ở đó.
  Giờ app tự làm mới như `claude` CLI (cùng endpoint, cùng file khoá) và ghi token mới vào `.credentials.json`. Chỉ làm khi token đã hết hạn.
  Bật mặc định; tắt bằng ô *Tự làm mới token hết hạn* trong Dashboard → tab Chung.
- Nếu server từ chối refresh token, app báo `phiên đăng nhập hết hạn - ... /login` và không gửi lại token đó nữa cho tới khi bạn đăng nhập lại.

## 1.0.1

Sửa lỗi dashboard: nút bấm không ăn / phải bấm nhiều lần, đóng chậm.

- Sửa: nút `refresh` / `save` phải bấm nhiều lần mới ăn. Thông báo nhỏ (toast) ở góc phải dưới sau khi mờ đi vẫn chặn chuột và đè lên hai nút này; giờ toast không bắt chuột nữa.
- Sửa: bấm nút ngay khi dashboard vừa mở (trước khi kết nối với app sẵn sàng) bị mất cú bấm. Giờ cú bấm được giữ lại và chạy ngay khi sẵn sàng.
- Sửa: nút `refresh` bị khoá mỗi khi app đang tự cập nhật nền. Giờ chỉ khoá trong lúc refresh do chính bạn bấm; cập nhật nền chỉ hiện vòng quay.
- Sửa: cửa sổ không còn tự đổi cỡ trong lúc đang nhấn chuột (nút dịch chỗ giữa lúc nhấn và nhả làm mất cú bấm).
- Sửa: góc kéo giãn ở góc phải trên đè một phần lên nút ✕; các nút trên thanh tiêu đề giờ luôn nằm trên.
- Sửa: đóng dashboard (✕ / `Esc` / Alt+F4) mất ~2 giây. Giờ cửa sổ biến mất và tiến trình kết thúc trong khoảng 20–40ms (đo trên máy dev).
- Sửa: `Esc` không đóng được khi con trỏ đang ở trong ô nhập.
- Sửa: đổi một tuỳ chọn rồi đóng ngay có thể làm mất thay đổi đó; giờ thay đổi đang chờ được lưu trước khi đóng.
- Sửa: `config.json` được ghi kiểu nguyên tử (file tạm + thay thế), không còn nguy cơ bị cắt dở làm mất cấu hình nếu tiến trình bị tắt giữa chừng.
- Sửa: đóng dashboard rồi mở lại ngay có thể không mở; mở dashboard không còn làm widget/tray đứng hình.
- Sửa: dashboard chớp / nhảy. Danh sách account không còn bị vẽ lại mỗi lần cập nhật (vòng quay và hiệu ứng nhấp nháy không còn giật); cửa sổ hiện ra một lần ở đúng cỡ thay vì hiện cỡ mặc định rồi nhảy; ở chế độ tự vừa, cửa sổ chỉ cao thêm khi cần và không tự co lại khi một dòng cảnh báo biến mất (co lại khi bạn đổi tab / bấm "tự vừa").
- Sửa: không kéo (di chuyển) được cửa sổ dashboard bằng thanh tiêu đề.
- Dữ liệu tạm của WebView2 giờ nằm trong `%LOCALAPPDATA%\claude-usage\webview\` và tự dọn, không còn để lại thư mục `tmp*\EBWebView` (~6MB mỗi lần mở) trong `%TEMP%`.

## 1.0.0

Bản đầu tiên.

- Theo dõi 5h / 7d của nhiều account Claude Code cùng lúc (tự tìm profile).
- Tray icon, widget taskbar (hai vòng + giờ reset, hỗ trợ taskbar ngang và dọc, DPI), dashboard kiểu terminal.
- Widget kéo thả / đổi cỡ / xoay, có khoá. 5 kiểu giao diện (vòng tròn, thanh ngang, thẻ số lớn, ô vuông terminal, đồng hồ bán nguyệt), đổi nhanh bằng chuột phải → Next style.
- Tuỳ chỉnh giao diện widget (tab riêng, áp dụng ngay): tên riêng / tên ngắn cho từng account, cỡ chữ, độ đậm, màu chữ, nền mờ, ẩn bớt account. Tên dài tự co chữ rồi cắt bằng …; kiểu vòng tròn đưa số % vào giữa và tên xuống dưới khi tên dài.
- Chọn thông tin hiển thị trên widget bằng ô tick (tên, 5h, 7d, giờ reset 5h / 7d), áp dụng cho cả 5 kiểu.
- Rê chuột vào account để xem thẻ chi tiết: 5h / 7d + giờ reset cụ thể, giới hạn riêng theo model, credit dùng thêm, trạng thái cập nhật. Dashboard cũng có hint tương tự khi rê vào từng dòng.
- Dashboard co giãn: kéo giãn từ mọi cạnh/góc, phóng to / khôi phục, bố cục nhiều cột → 1 cột theo bề ngang, cuộn bên trong khi cần (thanh cuộn chỉ hiện khi rê chuột), nhớ cỡ và vị trí, thanh tiến trình CSS co giãn được.
- Hỗ trợ nhiều profile: widget tự co cỡ rồi cắt thành ô "+N khác", sắp xếp theo mức dùng, giới hạn số account, nhãn tự sinh không trùng, chu kỳ poll tự giãn, toast gọn khi nhiều account.
- Chỉ báo "đang chờ": chấm xanh dương trên tray, vòng quay trên widget, spinner + nút refresh bận trên dashboard,
  và thông báo kết quả refresh (account nào được cập nhật, account nào bị bỏ qua và vì sao).
- Chống 429: tối thiểu 90s giữa các lần poll, lịch riêng từng account, lùi dần theo cấp số nhân hoặc `Retry-After`,
  giữ giá trị cũ khi lỗi, không bắn API ồ ạt khi khởi động lại, khoảng cách tối thiểu cho refresh thủ công.
- Thông báo có cấu hình: bật/tắt tổng, riêng cảnh báo / chặn, nút gửi thử.
  Mỗi chu kỳ chỉ báo 1 lần (so giờ reset có dung sai vì API dao động ~1s), nhớ qua lần khởi động lại,
  gộp thành 1 toast gọn, giới hạn tần suất. Lần chạy đầu chỉ ghi nhận tình trạng hiện tại, không báo ngay.
- Hook chặn prompt Claude Code CLI khi vượt ngưỡng (an toàn: lỗi/không chắc thì cho qua), cài / gỡ bằng 1 nút, không đụng cấu hình khác.
- Chạy cùng Windows (ẩn ở tray), một exe duy nhất, dashboard là tiến trình riêng nên RAM chỉ tốn lúc mở (~63MB khi chạy nền).
- Cache ghi nguyên tử và có thử lại (Windows từ chối ghi đè file đang được đọc), nhật ký sự kiện `events.log`.
