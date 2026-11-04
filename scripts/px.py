"""采样截图像素颜色：python px.py 图.png x,y x,y ..."""

import sys

from PIL import Image

im = Image.open(sys.argv[1]).convert("RGB")
for a in sys.argv[2:]:
    x, y = a.split(",")
    print(a, im.getpixel((int(x), int(y))))
