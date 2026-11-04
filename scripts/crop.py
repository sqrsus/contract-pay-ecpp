"""裁剪截图局部并放大，便于看细节：python crop.py 图.png x0,y0,x1,y1 [倍数]"""

import sys

from PIL import Image

src, box = sys.argv[1], sys.argv[2]
scale = int(sys.argv[3]) if len(sys.argv) > 3 else 2
x0, y0, x1, y1 = (int(v) for v in box.split(","))
im = Image.open(src).convert("RGB").crop((x0, y0, x1, y1))
im = im.resize((im.width * scale, im.height * scale), Image.NEAREST)
out = src.replace(".png", "_crop.png")
im.save(out)
print(out, im.size)
