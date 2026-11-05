"""跑一次 GUI 并保存截图（CTP_SHOT 走渲染器截图通道，不依赖窗口是否在前台）。

用法：python scripts/shot.py <输出.png> [KEY=VALUE ...]
"""

import os
import subprocess
import sys

from PIL import Image

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

DIST = r"D:\pay2\dist"
EXE = os.environ.get("CTP_SHOT_EXE", os.path.join(DIST, "ctp-gui.exe"))


def main() -> int:
    if len(sys.argv) < 2:
        print("用法：python shot.py <输出.png> [KEY=VALUE ...]")
        return 2
    out = sys.argv[1]
    ppm = os.path.join(DIST, "_shot_tmp.ppm")
    if os.path.exists(ppm):
        os.remove(ppm)

    env = dict(os.environ)
    env["CTP_SHOT"] = ppm
    env.setdefault("CTP_SHOT_FRAME", "60")
    for pair in sys.argv[2:]:
        k, _, v = pair.partition("=")
        env[k] = v

    proc = subprocess.run([EXE], cwd=os.path.dirname(EXE), env=env, timeout=120)
    if not os.path.exists(ppm):
        print(f"截图失败：未生成 {ppm}（exit={proc.returncode}）")
        return 1
    img = Image.open(ppm)
    img.save(out)
    print(f"saved {out} {img.size}")
    os.remove(ppm)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
