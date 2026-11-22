# claude-usage: hướng dẫn cho AI

**Đọc `HANDOFF.md` trước** (kiến trúc, bản đồ file, test, bẫy). File này chỉ nhắc những quy tắc quan trọng nhất.

- App Windows (Python 3.14 + pystray + Pillow + pywebview, PyInstaller onedir: `dist\claude-usage.exe` + `dist\_internal`). **Đang port sang Rust: đọc `PORT_PLAN.md`** theo dõi hạn mức 5h/7d của nhiều account Claude Code.
- Giao tiếp với người dùng **bằng tiếng Việt**, ngắn gọn, báo cáo trung thực (cái gì test thật / giả lập / chưa kiểm chứng).
- Lệnh chính: `.\.venv\Scripts\python tests\test_unit.py` (~40s) · `powershell -ExecutionPolicy Bypass -File build.ps1` (test + build + đóng gói) ·
  `.\.venv\Scripts\python tests\smoke_release.py` (kiểm tra gói). Chỉ chạy **một** test GUI mỗi lần.
- Test luôn cô lập dữ liệu bằng biến môi trường `LOCALAPPDATA` trỏ thư mục tạm; không để test ghi vào `%LOCALAPPDATA%\claude-usage` thật.
- **Không** đưa token/credential/PII vào code, log, commit, ảnh chụp. App đọc token của các profile và **chỉ ghi** `.credentials.json`
  trong `core.refresh_if_expired()` (tự làm mới token đã hết hạn, như CLI). Token không bao giờ bị in/log/gửi đi đâu ngoài endpoint của Anthropic.
- **Hỏi trước** khi: cài hook vào `settings.json` thật, đăng release ra ngoài, xoá/ghi đè dữ liệu người dùng.
- Bẫy hay gặp: viết `"\n"` qua heredoc/tham số lệnh sẽ thành xuống dòng thật (dùng Edit/Write hoặc `chr(10)`, rồi `py_compile`);
  không gắn đối tượng `Window` vào thuộc tính **công khai** của `js_api` (dùng `_window`); `ImageDraw` trên RGBA ghi đè pixel;
  đừng chụp màn hình khi cửa sổ có thể nằm sau cửa sổ riêng tư của người dùng. Chi tiết ở `HANDOFF.md` mục 9.
- Sau khi sửa: cập nhật `README.md`, `CHANGELOG.md`, `HANDOFF.md`.
