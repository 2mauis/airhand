"""生成 Android 的位图图标：旧版（Android 7.x）启动图标和电视桌面的横幅。

Android 8+ 的自适应图标是矢量图（res/drawable/ic_launcher_*.xml），不需要这里。
旧版图标直接缩放桌面客户端的 client/src-tauri/icons/icon.png（源文件是 assets/icon.svg）。

用法（在仓库根目录）：python android/scripts/gen_icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
RES = ROOT / "android/app/src/main/res"
ICON = ROOT / "client/src-tauri/icons/icon.png"

SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}


def gradient(w: int, h: int) -> Image.Image:
    """与图标相同的 135° 三色渐变"""
    stops = [(0.0, (0x7B, 0x5C, 0xFF)), (0.52, (0x4C, 0x7D, 0xFF)), (1.0, (0x12, 0xD3, 0xEE))]
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / (w - 1) + y / (h - 1)) / 2
            for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
                if t <= t1:
                    k = (t - t0) / (t1 - t0)
                    px[x, y] = tuple(round(a + (b - a) * k) for a, b in zip(c0, c1))
                    break
    return img


def cursor_mark(size: int) -> Image.Image:
    """白色的信号波 + 光标（坐标取自 assets/icon.svg 的 1024 画布）"""
    s = 4  # 超采样
    big = Image.new("RGBA", (1024 * s // 2, 1024 * s // 2), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    k = s / 2

    def p(x, y):
        return (x * k, y * k)

    cx, cy = 484, 409
    for r, alpha in ((154, 242), (258, 153)):
        box = [p(cx - r, cy - r), p(cx + r, cy + r)]
        d.arc(box, 180, 270, fill=(255, 255, 255, alpha), width=round(54 * k))
    poly = [p(484, 409), p(484, 783), p(583, 700), p(645, 846), p(708, 820), p(645, 674), p(770, 674)]
    d.polygon(poly, fill=(255, 255, 255, 255))
    d.line(poly + [poly[0]], fill=(255, 255, 255, 255), width=round(44 * k), joint="curve")
    # 只保留标志所在的区域
    box = (round(200 * k), round(130 * k), round(800 * k), round(870 * k))
    return big.crop(box).resize((size * 600 // 740, size), Image.LANCZOS)


def main() -> None:
    icon = Image.open(ICON).convert("RGBA")
    for density, px in SIZES.items():
        out = RES / f"mipmap-{density}"
        out.mkdir(parents=True, exist_ok=True)
        icon.resize((px, px), Image.LANCZOS).save(out / "ic_launcher.png")

    # 电视横幅：320×180（xhdpi），渐变底 + 标志 + 名称
    w, h = 320, 180
    banner = gradient(w, h).convert("RGBA")
    mark = cursor_mark(96)
    banner.alpha_composite(mark, (34, (h - mark.height) // 2))
    d = ImageDraw.Draw(banner)
    font = None
    for name in ("segoeuib.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"):
        try:
            font = ImageFont.truetype(name, 46)
            break
        except OSError:
            continue
    font = font or ImageFont.load_default()
    text = "Airhand"
    tw = d.textlength(text, font=font)
    x = 34 + mark.width + 18
    x = min(x, w - tw - 16)
    d.text((x, h / 2), text, font=font, fill=(255, 255, 255, 255), anchor="lm")
    out = RES / "drawable-xhdpi"
    out.mkdir(parents=True, exist_ok=True)
    banner.convert("RGB").save(out / "tv_banner.png")
    print("ok")


if __name__ == "__main__":
    main()
