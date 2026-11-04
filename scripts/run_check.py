"""启动 GUI，等若干秒后报告进程是否存活、退出码与 stderr。"""

import os
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

DIST = r"D:\pay2\dist"
exe = os.path.join(DIST, "ctp-gui.exe")
env = dict(os.environ)
env["CTP_SELECT"] = "1"
env["CTP_SCREEN"] = "contracts"

out = open(os.path.join(DIST, "_o.txt"), "wb")
err = open(os.path.join(DIST, "_e.txt"), "wb")
p = subprocess.Popen([exe], cwd=DIST, stdout=out, stderr=err, env=env)
for i in range(6):
    time.sleep(1)
    print(i + 1, "s poll=", p.poll())
p.terminate()
try:
    p.wait(timeout=3)
except subprocess.TimeoutExpired:
    p.kill()
out.close()
err.close()

for name in ("_e.txt", "_o.txt"):
    path = os.path.join(DIST, name)
    data = open(path, "rb").read()
    print(name, len(data))
    if data:
        print(data.decode("utf-8", "replace")[:3000])
