"""交叉验证：把窗口切到前台后直接截屏（不走 PrintWindow）。"""

import ctypes
import ctypes.wintypes
import os
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TITLE = "工程合同台账与拨款管理平台"
DIST = r"D:\pay2\dist"
user32 = ctypes.windll.user32


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


env = dict(os.environ)
env["CTP_SCREEN"] = "payments"
err = open(os.path.join(DIST, "_e2.txt"), "wb")
p = subprocess.Popen([os.path.join(DIST, "ctp-gui.exe")], cwd=DIST, stderr=err, env=env)

hwnd = 0
for _ in range(40):
    hwnd = user32.FindWindowW(None, TITLE)
    if hwnd:
        break
    time.sleep(0.3)
print("hwnd", hwnd)

ok = False
for _ in range(10):
    user32.ShowWindow(hwnd, 6)
    time.sleep(0.15)
    user32.ShowWindow(hwnd, 9)
    user32.SetForegroundWindow(hwnd)
    user32.BringWindowToTop(hwnd)
    time.sleep(0.4)
    if user32.GetForegroundWindow() == hwnd:
        ok = True
        break
print("foreground", ok)
time.sleep(2.0)

rect = RECT()
user32.GetWindowRect(hwnd, ctypes.byref(rect))
print("rect", rect.left, rect.top, rect.right, rect.bottom)

from PIL import ImageGrab

img = ImageGrab.grab(
    bbox=(rect.left, rect.top, rect.right, rect.bottom), all_screens=True
)
out = r"D:\pay2\shots\99-screen.png"
img.save(out)
print("saved", out, img.size)

p.terminate()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.kill()
err.close()
