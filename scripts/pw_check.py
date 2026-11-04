"""对照实验：用同一个 PrintWindow 抓一个已知正常的窗口（记事本），判断抓图通道是否失效。"""

import ctypes
import os
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gui_probe import capture, find_window  # noqa: E402

user32 = ctypes.windll.user32


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


p = subprocess.Popen(["notepad.exe"])
hwnd = 0
for _ in range(40):
    hwnd = user32.FindWindowW("Notepad", None)
    if hwnd:
        break
    time.sleep(0.25)
print("notepad hwnd", hwnd)
time.sleep(1.5)

rect = RECT()
user32.GetWindowRect(hwnd, ctypes.byref(rect))
print("rect", rect.left, rect.top, rect.right, rect.bottom)

out = r"D:\pay2\shots\99-notepad.png"
capture(hwnd, out)

from PIL import Image

im = Image.open(out).convert("RGB")
w, h = im.size
colors = {im.getpixel((x, y)) for x in range(0, w, 7) for y in range(0, h, 7)}
print("distinct_colors", len(colors), sorted(colors)[:4])

p.terminate()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.kill()
