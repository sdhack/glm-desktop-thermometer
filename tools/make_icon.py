from PIL import Image, ImageDraw, ImageFont
import math

S = 8  # 超采样倍数（抗锯齿）
N = 256
W = N * S

img = Image.new('RGBA', (W, W), (0, 0, 0, 0))
d = ImageDraw.Draw(img)


def rr(xy, r, fill=None, outline=None, width=1):
    d.rounded_rectangle([v * S for v in xy], radius=r * S, fill=fill, outline=outline, width=int(width * S))


def circle(c, r, fill=None, outline=None, width=1):
    d.ellipse([(c[0] - r) * S, (c[1] - r) * S, (c[0] + r) * S, (c[1] + r) * S],
              fill=fill, outline=outline, width=int(width * S))


# ── 背景：深色圆角方块 + 对角渐变 + 细描边 ──
BG_R = 52
bg = Image.new('RGBA', (W, W), (0, 0, 0, 0))
bgm = Image.new('L', (W, W), 0)
dm = ImageDraw.Draw(bgm)
dm.rounded_rectangle([6 * S, 6 * S, (N - 6) * S, (N - 6) * S], radius=BG_R * S, fill=255)

# 渐变：左上深蓝灰 → 右下暗青
grad = Image.new('RGBA', (W, W))
gp = grad.load()
for y in range(W):
    t = y / W
    for x in range(W):
        tx = x / W
        tt = (t * 0.75 + tx * 0.25)
        r = int(0x22 + (0x0E - 0x22) * tt)
        g = int(0x2A + (0x1C - 0x2A) * tt)
        b = int(0x3E + (0x2E - 0x3E) * tt)
        gp[x, y] = (r, g, b, 255)
bg.paste(grad, (0, 0), bgm)
img.alpha_composite(bg)
# 细描边
rr((6, 6, N - 6, N - 6), BG_R, outline=(255, 255, 255, 36), width=2.2)

# ── 温度计（居中偏左，竖直）──
cx = 118
tube_w = 26           # 玻璃管外径
tube_top = 46
bulb_c = (cx, 176)    # 感温泡中心
bulb_r = 30
mercury_top = 74      # 水银柱顶（越低越热）

# 玻璃管（半透明白）
d.rounded_rectangle(
    [(cx - tube_w / 2) * S, tube_top * S, (cx + tube_w / 2) * S, (bulb_c[1] + 10) * S],
    radius=tube_w / 2 * S, fill=(255, 255, 255, 38), outline=(255, 255, 255, 110),
    width=int(2.5 * S))

# 感温泡（玻璃）
circle(bulb_c, bulb_r, fill=(255, 255, 255, 38), outline=(255, 255, 255, 110), width=int(2.5 * S))

# 水银：底部热红 → 顶部凉青 渐变柱
merc_w = 14
merc = Image.new('RGBA', (W, W), (0, 0, 0, 0))
mp = merc.load()
top = int(mercury_top * S)
bot = int((bulb_c[1] + 6) * S)
mw = int(merc_w * S)
mx = int((cx - merc_w / 2) * S)
for y in range(top, bot):
    t = (y - top) / max(1, bot - top)  # 0 顶 → 1 底
    # 青绿 → 黄 → 橙红
    if t < 0.5:
        k = t / 0.5
        r = int(0x3A + (0xFF - 0x3A) * k)
        g = int(0xD0 + (0xC4 - 0xD0) * k)
        b = int(0x6A + (0x2E - 0x6A) * k)
    else:
        k = (t - 0.5) / 0.5
        r = 255
        g = int(0xC4 + (0x45 - 0xC4) * k)
        b = int(0x2E + (0x2E - 0x2E) * k)
    for x in range(mx, mx + mw):
        mp[x, y] = (r, g, b, 235)
# 水银柱圆头
mc = (cx, mercury_top + merc_w / 2)
circle(mc, merc_w / 2, fill=(0x3A, 0xD0, 0x6A, 235))
# 感温泡内的水银球
circle(bulb_c, bulb_r - 8, fill=(255, 0x45, 0x2E, 240))
# 高光
d.rounded_rectangle([(cx - 3) * S, (mercury_top + 8) * S, (cx - 1) * S, (bulb_c[1] - 14) * S],
                    radius=1 * S, fill=(255, 255, 255, 90))
circle((bulb_c[0] - 9, bulb_c[1] - 10), 6, fill=(255, 255, 255, 80))

img.alpha_composite(merc)

# ── 刻度线（右侧三道）──
for ty, wl in [(96, 14), (128, 20), (160, 14)]:
    d.rounded_rectangle([(cx + tube_w / 2 + 10) * S, ty * S, (cx + tube_w / 2 + 10 + wl) * S,
                         (ty + 4) * S], radius=2 * S, fill=(255, 255, 255, 95))

# ── 右上角状态点（绿→产品健康灯概念）──
# 深色描边（先画）再压绿色实心点，保证绿色醒目
circle((206, 52), 14, fill=(10, 12, 18, 220))
circle((206, 52), 11, fill=(0x36, 0xD9, 0x45, 255))
circle((203, 48), 4, fill=(255, 255, 255, 160))

# ── 输出 ──
img = img.resize((N, N), Image.LANCZOS)
img.save('icon_256.png')

sizes = [256, 128, 64, 48, 32, 16]
imgs = [img]
for sz in sizes[1:]:
    imgs.append(img.resize((sz, sz), Image.LANCZOS))
imgs[0].save('tempmon.ico', sizes=[(s, s) for s in sizes], append_images=imgs[1:])
print('icon generated: tempmon.ico + icon_256.png')
