"""启动 GUI 并截图。

用 PrintWindow 直接把窗口内容渲染到位图，不依赖窗口是否在最前，
因此不会抢占前台、也不会干扰其它程序。

用法：
    python gui_probe.py 输出.png [选项 ...]

选项：
    screen=payments        启动页（CTP_SCREEN）
    theme=dark             启动主题（CTP_THEME）
    dialog=new_contract    启动即打开弹窗（CTP_DIALOG）
    size=1040x700          窗口尺寸
    x,y                    窗口内逻辑坐标点击（仅在窗口已在前台时执行）
"""

import ctypes
import ctypes.wintypes
import os
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

TITLE = "工程合同台账与拨款管理平台"
DIST = r"D:\pay2\dist"
EXE = os.path.join(DIST, "ctp-gui.exe")

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32

PW_RENDERFULLCONTENT = 0x00000002


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", ctypes.c_uint32),
        ("biWidth", ctypes.c_int32),
        ("biHeight", ctypes.c_int32),
        ("biPlanes", ctypes.c_uint16),
        ("biBitCount", ctypes.c_uint16),
        ("biCompression", ctypes.c_uint32),
        ("biSizeImage", ctypes.c_uint32),
        ("biXPelsPerMeter", ctypes.c_int32),
        ("biYPelsPerMeter", ctypes.c_int32),
        ("biClrUsed", ctypes.c_uint32),
        ("biClrImportant", ctypes.c_uint32),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", ctypes.c_uint32 * 3)]


def find_window(timeout=15.0):
    end = time.time() + timeout
    while time.time() < end:
        hwnd = user32.FindWindowW(None, TITLE)
        if hwnd:
            return hwnd
        time.sleep(0.3)
    return 0


def capture(hwnd, out):
    """把整个窗口（含标题栏与边框）渲染成 PNG。"""
    from PIL import Image

    rect = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w, h = rect.right - rect.left, rect.bottom - rect.top
    if w <= 0 or h <= 0:
        raise RuntimeError(f"窗口尺寸异常：{w}x{h}")

    hdc_win = user32.GetWindowDC(hwnd)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_win)
    hbmp = gdi32.CreateCompatibleBitmap(hdc_win, w, h)
    gdi32.SelectObject(hdc_mem, hbmp)
    ok = user32.PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT)

    bi = BITMAPINFO()
    bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h  # 负数 = 自上而下
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    bi.bmiHeader.biCompression = 0  # BI_RGB

    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(hdc_mem, hbmp, 0, h, buf, ctypes.byref(bi), 0)

    img = Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")
    img.save(out)
    print("printwindow", ok, "size", (w, h), "->", out)

    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(hwnd, hdc_win)


def capture_screen(hwnd, out):
    """从屏幕直接抓窗口矩形。

    PrintWindow 对 GPU 呈现的窗口（DirectX / wgpu）经常返回空白，
    这时把窗口置顶后用 BitBlt 拷屏幕像素，拿到的是用户真实看到的画面。
    """
    from PIL import Image

    rect = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w, h = rect.right - rect.left, rect.bottom - rect.top
    if w <= 0 or h <= 0:
        raise RuntimeError(f"窗口尺寸异常：{w}x{h}")

    HWND_TOPMOST = -1
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW = 0x0002, 0x0001, 0x0040
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW)
    user32.SetForegroundWindow(hwnd)
    time.sleep(1.2)

    hdc_screen = user32.GetDC(0)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
    hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, w, h)
    gdi32.SelectObject(hdc_mem, hbmp)
    SRCCOPY = 0x00CC0020
    ok = gdi32.BitBlt(hdc_mem, 0, 0, w, h, hdc_screen, rect.left, rect.top, SRCCOPY)

    bi = BITMAPINFO()
    bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    bi.bmiHeader.biCompression = 0

    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(hdc_mem, hbmp, 0, h, buf, ctypes.byref(bi), 0)
    img = Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")
    img.save(out)
    print("screengrab", ok, "size", (w, h), "->", out)

    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(0, hdc_screen)


def click(hwnd, lx, ly, scale):
    rect = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    x = int(rect.left + lx * scale)
    y = int(rect.top + ly * scale)
    user32.SetCursorPos(x, y)
    time.sleep(0.15)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.8)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(DIST, "_shot.png")
    clicks = []
    env = dict(os.environ)
    size = None
    grab = "printwindow"
    for a in sys.argv[2:]:
        if a.startswith("grab="):
            grab = a.split("=", 1)[1]
            continue
        if "=" in a and a.split("=", 1)[0] in ("screen", "theme", "dialog"):
            k, v = a.split("=", 1)
            env["CTP_" + k.upper()] = v
            continue
        if a.startswith("size="):
            w, h = a.split("=", 1)[1].split("x")
            size = (int(w), int(h))
            continue
        x, y = a.split(",")
        clicks.append((float(x), float(y)))

    err = open(os.path.join(DIST, "_err.txt"), "wb")
    p = subprocess.Popen([EXE], cwd=DIST, stdout=subprocess.DEVNULL, stderr=err, env=env)
    hwnd = find_window()
    print("hwnd", hwnd, "alive", p.poll() is None)
    if not hwnd:
        print("window not found")
        return

    if size:
        scale0 = 1.0
        try:
            scale0 = user32.GetDpiForWindow(hwnd) / 96.0
        except Exception:  # noqa: BLE001
            pass
        user32.SetWindowPos(
            hwnd, 0, 20, 20, int(size[0] * scale0), int(size[1] * scale0), 0
        )
        time.sleep(1.0)

    try:
        scale = user32.GetDpiForWindow(hwnd) / 96.0
    except Exception:  # noqa: BLE001
        scale = 1.0

    # 只有在窗口确实位于前台时才模拟点击，避免点到别的程序
    fg = False
    for _ in range(4):
        user32.SetForegroundWindow(hwnd)
        time.sleep(0.3)
        if user32.GetForegroundWindow() == hwnd:
            fg = True
            break
    if clicks and not fg:
        print("skip clicks: window not foreground")
        clicks = []
    for (x, y) in clicks:
        click(hwnd, x, y, scale)

    # 强制重绘：窗口未获得焦点时 eframe 可能不刷新，PrintWindow 会拿到空白
    rect = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    user32.SetWindowPos(
        hwnd,
        0,
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top + 1,
        0x0004,
    )
    user32.InvalidateRect(hwnd, None, True)
    user32.UpdateWindow(hwnd)
    time.sleep(0.6)

    time.sleep(2.4)
    if grab == "screen":
        capture_screen(hwnd, out)
    else:
        capture(hwnd, out)

    p.terminate()
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
    err.close()


if __name__ == "__main__":
    main()
