# Phase 0: số đo mốc (bản Python 1.1.0)

Máy: máy phát triển của chủ dự án, Windows 11, ngày 2026-11-19. Mọi phép đo dùng `LOCALAPPDATA` sandbox, dữ liệu giả.
Số liệu có nhiễu (máy đang dùng); lấy median.

## Hook (`echo {} | <exe> hook`, 10 lần, đo từ Git Bash)
| Bản | Median | Ghi chú |
|---|---|---|
| onefile (`dist\claude-usage.exe` cũ) | **~840ms** | giải nén `_MEI` mỗi lần |
| onedir (từ 2026-11-19) | **~232ms** | 1 lần ngoại lệ 1.1s (khởi động lạnh) |
| Python source (`python -c promptguard`) | ~115ms | |
| Chỉ shell (`bash -c 'exit 0'`) | ~41ms | sàn của mọi hook chạy qua bash |

## Mở dashboard (start `<exe> dashboard` → cửa sổ "claude-usage" hiện), `bench\bench_dashboard.ps1`
| Bản | Lần chạy |
|---|---|
| onefile | 1647 / 1548 / 1564 ms |
| onedir | 2040 / 762 / 1229 ms |
| Python source | 1347 / 1294 / 1262 ms |

Từ tray/exe lần 2 còn cộng thêm ≤1s chờ vòng loop thấy `show.flag` (smoke đo 2.2s tổng).
→ Phần lớn là khởi động pywebview + WebView2, không phải giải nén.

## Vẽ widget (không cửa sổ), `bench\bench_widget.py`: `render`+`compose`, median
| Kiểu | 4 profile | 14 profile |
|---|---|---|
| rings | 7.6ms | 57ms |
| bars | 11.9ms | 89ms |
| cards | 12.0ms | 107ms |
| segments | 9.1ms | 90ms |
| arc | 7.5ms | 65ms |

`plan_layout` 1–4ms, `load_cache` ~0.05ms, `light_taskbar` ~0.01ms (trường hợp bình thường, không đụng file đang ghi).
→ Với 4 profile widget không phải nguồn lag chính. Với ≥10 profile, frame ~ bằng chu kỳ timer 110ms → giật khi spinner/kéo.
Chưa đo: kéo resize widget bằng chuột thật (review dự đoán là chỗ giật nhất vì mỗi lần di chuột đều đo lại layout).

## RAM
Daemon onefile: bootloader ~8MB + tiến trình thật ~41MB. Onedir: 1 tiến trình ~55MB (vừa khởi động).

## Mục tiêu bản Rust
Hook < 60ms end-to-end (sàn bash ~41ms), mở dashboard < 600ms, frame widget < 2ms với 14 profile, RAM nền < 20MB.
