"""Generate assets/icon.ico and assets/version_info.txt for the PyInstaller build."""
import sys
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
import core

A = ROOT / "assets"; A.mkdir(exist_ok=True)


def icon(S=1024):
    """Dark rounded tile with the widget's two rings: outer = 5h (green), inner = 7d (amber)."""
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0)); d = ImageDraw.Draw(img)
    d.rounded_rectangle((S * .03, S * .03, S * .97, S * .97), radius=S * .22, fill="#121212", outline="#2e2e2e", width=int(S * .012))
    c = S / 2
    for r, col, pct, w in ((S * .34, "#7fd88f", .72, S * .085), (S * .20, "#f5a742", .46, S * .085)):
        box = (c - r, c - r, c + r, c + r)
        d.ellipse(box, outline="#2c2c2c", width=int(w))
        d.arc(box, -90, -90 + 360 * pct, fill=col, width=int(w))
    d.ellipse((c - S * .045, c - S * .045, c + S * .045, c + S * .045), fill="#fab283")
    return img


big = icon()
big.resize((256, 256), Image.LANCZOS).save(A / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
big.resize((512, 512), Image.LANCZOS).save(A / "icon.png")

v = tuple(int(x) for x in core.__version__.split(".")) + (0,)
(A / "version_info.txt").write_text(f"""# UTF-8
VSVersionInfo(
  ffi=FixedFileInfo(filevers={v}, prodvers={v}, mask=0x3f, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[
    StringFileInfo([StringTable('040904B0', [
      StringStruct('CompanyName', ''),
      StringStruct('FileDescription', 'Claude usage'),
      StringStruct('FileVersion', '{core.__version__}'),
      StringStruct('InternalName', 'claude-usage'),
      StringStruct('OriginalFilename', 'claude-usage.exe'),
      StringStruct('ProductName', 'claude-usage'),
      StringStruct('ProductVersion', '{core.__version__}')])]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
""", "utf-8")
print("assets written:", sorted(p.name for p in A.iterdir()))
