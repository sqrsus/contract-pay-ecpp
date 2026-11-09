# 龙芯麒麟 Linux（loongarch64 / ABI 1.0）构建与运行可行性评估

> 评估对象：工程合同台账与拨款管理平台 v0.3.2（4 crate / 约 20,000 行 Rust；评估时基线，当前代码已到 0.3.6）
> 评估方式：仓库静态审查 + 本机 rustc 1.99 实测（目标 std、ELF 符号版本）
> 评估日期：2026-11-09
> 说明：本机无 loongarch64 硬件、无 LoongArch 交叉 C 编译器，**未做真实构建**；凡未实测处均在第十节标注为假设。

---

## 0. 结论速览（分层）

| 层次 | 组件 | 结论 | 风险 |
|---|---|---|---|
| 领域层 | `ctp-core`（金额 / 比率 / 合同 / 额度） | **可直接编译运行**，零第三方依赖（仅 thiserror） | 低 |
| 数据层 | `ctp-store`（SQLite + 视图） | **可编译运行**，`rusqlite` bundled 需目标机 C 编译器 | 低 |
| 应用层 | `ctp-app`（合同 / 拨款 / 报表 / Excel） | **可编译运行**，纯 Rust 依赖（rust_xlsxwriter / calamine） | 低 |
| 应用层·打印 | `ctp-app::gdi`（GDI 出纸 + 位图预览） | **不可用**（Windows 专属），已 `cfg` 降级为「打印功能仅支持 Windows」的明确报错，**不崩溃** | 高（功能缺失） |
| 界面层 | `ctp-gui`（egui/eframe） | **可编译**；运行需 OpenGL 3.3+ / X11 或 Wayland / 一个可用的文件对话框后端 | 中 |
| 构建分发 | Cargo + rustc | **可原生或交叉构建**，但受系统 ABI 与 glibc 版本硬约束 | **高** |

**一句话结论**：这个项目的**代码可移植性明显好于同类 Windows 桌面程序**——全部 `unsafe` 与 FFI 只集中在一个 `gdi.rs`（`external` 面为零），版面绘制早已抽象成 `Canvas` trait，字体候选表已含 Linux 路径，`open_external` 也已有 `xdg-open` 分支。真正的门槛**不在 Rust 代码，而在两处系统层**：① ABI 1.0（旧世界）+ glibc 2.28 与上游 Rust 工具链（新世界 / glibc 2.36）的代差；② 打印输出通道必须从 GDI 换成 PDF + CUPS。

**建议的决策顺序**：先花 30 分钟在目标机采集 5 个事实（第 8 节第 0 关），再决定走「原生编译直上」还是「换镜像 / 用厂商工具链」。在事实未明前不要投入打印适配——那是唯一需要新写代码的模块。

---

## 1. 技术画像

### 1.1 语言与规模

| 项 | 值 |
|---|---|
| 语言 | Rust，edition 2021 |
| 结构 | Cargo workspace，4 个 crate，`resolver = "2"` |
| 代码量 | core 3,058 / store 2,262 / app 5,590 / gui 9,081 ≈ **19,991 行** |
| 可执行产物 | **仅 1 个 bin：`ctp-gui`**（GUI 子系统，无 CLI） |
| 构建脚本 | **无 `build.rs`**，**无 `.cargo/config.toml`**（交叉编译需新建） |
| 依赖 | `Cargo.lock` 提交，**452 个包**（含全平台条件依赖） |
| 版本真源 | 根 `Cargo.toml` 的 `workspace.package.version = 0.3.2` |

### 1.2 依赖分层（按移植难度）

| 类别 | 依赖 | 移植影响 |
|---|---|---|
| 纯 Rust，零 C | `thiserror` `serde_json` `rust_xlsxwriter 0.102` `calamine 0.38`（`default-features=false`）`zip 9`（`flate2`→`miniz_oxide` / `zlib-rs`，均纯 Rust） | 无风险，与架构无关 |
| 需要 C 编译器 | `libsqlite3-sys 0.30`（`rusqlite 0.32` → `bundled`，编译 SQLite 合并版 `sqlite3.c`） | 目标机需 gcc；SQLite 官方支持 LoongArch，无汇编优化也保证正确性 |
| 需要系统图形库（编译期头文件 / 运行期 dlopen） | `eframe 0.29`（`glow 0.13/0.14` + `glutin 0.32` + `winit 0.30`）`egui_extras 0.29` | 编译需 pkg-config + X11/Wayland/xkbcommon；运行需 GL 3.3 或 GLES 2 |
| 需要系统服务（D-Bus） | `rfd 0.14`（默认 `xdg-portal` → `ashpd 0.8` → `zbus`，纯 Rust 实现 D-Bus） | **lock 中无 gtk/glib**，即没有 gtk3 后端；依赖 `xdg-desktop-portal` 进程存在 |
| 平台专属（已门控） | `windows-sys 0.59`（`[target."cfg(windows)".dependencies]`） | Linux 不参与编译 |
| 已做跨平台分支 | `raw-window-handle 0.6`、字体候选表、`xdg-open` | 已适配，见 2.3 / 2.4 |

### 1.3 FFI 面（关键发现）

全仓库 `unsafe` / `extern "C"` / `#[link]` **仅出现在 `crates/ctp-app/src/gdi.rs` 一处**（`CreateFontW`、`TextOut`、`StartDocW`、`PrintDlgW` 等）。

`crates/ctp-app/src/lib.rs` 已有对称门面：

```rust
#[cfg(windows)]      pub fn render_print_page(..) -> Result<Preview, PrintError> { gdi::render_page(..) }
#[cfg(not(windows))] pub fn render_print_page(..) -> Result<Preview, PrintError> {
    Err(PrintError::Print("打印功能仅支持 Windows".into()))
}
```

**含义**：Rust 侧与系统 libc 的交互被压缩到 `std` 自身 + `libsqlite3` + X11/GL 的 dlopen 调用（参数基本是整数与指针）。这既降低了 ABI 差异的暴露面，也意味着**换架构只需换链接器与系统库，业务代码零改动**。

---

## 2. 逐项兼容性评估

### 2.1 Rust 工具链与目标

| 检查项 | 实测结果（本机 rustc 1.99.0，LLVM 23.1.1） |
|---|---|
| 目标是否被 LLVM 支持 | ✅ `rustc --print target-list` 含 `loongarch64-unknown-linux-gnu`、`-musl` |
| 预编译 std 是否可得 | ✅ `rustup target add loongarch64-unknown-linux-gnu` **成功**（已装入本机 sysroot） |
| 预编译 std 的 glibc 底线 | ⚠️ `libstd-06b96a174e26503c.so` 的 `.gnu.version_r` 只出现 **`GLIBC_2.36`**；静态链接的 `.rlib` 中不含版本串（版本需求在最终链接时生成） |

**推断（需实测确认）**：这套 std 构建在 **glibc 2.36（新世界基线）** 之上。若目标机是 glibc 2.28（麒麟 V10 早期），可能表现为链接期 `undefined reference to 'xxx@GLIBC_2.36'`，或运行期 `version 'GLIBC_2.36' not found`。

### 2.2 ABI 1.0 这个前提的含义

LoongArch 存在「旧世界（ABI 1.0）」与「新世界（ABI 2.0）」两套用户态 ABI，ELF 头 `e_flags` 里直接标注 ABI 版本。上游 Rust、Debian loong64 移植、mainline GCC 13+ **均面向新世界**。因此用户给定的 **ABI 1.0 是本次评估最重要的风险源**：

- 交叉编译**不能**解决它——换链接器只换链接阶段，Rust 代码的调用约定由 rustc/LLVM 决定，与用哪个 gcc 链接无关。
- `-Zbuild-std` 也不能改变调用约定（只改变 std 的编译基线）。
- 唯一有效路径是**使用与系统同代的工具链**（厂商 rustc）或**把系统升到新世界**。

### 2.3 界面层

| 检查项 | 结论 |
|---|---|
| `eframe 0.29` 后端 | 默认 `glow`（OpenGL）。egui 需 GL 3.3 core 或 GLES 2；llvmpipe（Mesa 软件渲染）通常可满足，但帧率低 |
| 窗口系统 | winit 0.30 支持 X11 与 Wayland；麒麟 V10 默认 UKUI + X11，可行 |
| 中文显示 | `crates/ctp-gui/src/fonts.rs` 已按优先级列了 **Noto CJK `ttc` / 文泉驿 microhei / arphic uming / PingFang**；找不到则界面全方块（`install()` 返回 `None`，不 panic） |
| 中文字体依赖 | 需确认目标机装有 `fonts-noto-cjk` 或 `fonts-wqy-microhei`；`NotoSansCJK-Regular.ttc` 路径取自 Debian 布局，麒麟可能落在 `/usr/share/fonts/…/NotoSansCJK-Regular.ttc`（同路径，通常命中） |
| 中文输入法 | ⚠️ 未实证。egui 的 IME 依赖 winit 的 IME 事件；Wayland 下走 `text-input` 协议较可靠，X11 下可能只能逐字上屏甚至不响应。**本系统必填中文（合同名称、承包单位），必须实测** |
| DPI / 缩放 | egui 自带 `pixels_per_point`，一般无需改；4K 屏需实机确认 |

### 2.4 文件对话框（导出 Excel 的必经路径）

`rfd 0.14` 在本 lock 中**只有 xdg-portal 后端**（`ashpd 0.8.2` + `zbus`，`Cargo.lock` 无 `gtk`/`glib`）。

- 若目标机运行着 `xdg-desktop-portal`（及 `-gtk`/`-ukui` 实现），保存框正常。
- 若没有该服务，`rfd::FileDialog::save_file()` 返回 `None`，而本项目把 `None` 当作**用户取消**处理（`ok_or_else(String::new)` → 静默）——**用户会看到「点了导出但什么都没发生」**，这是一个隐蔽的体验缺陷，需要额外兜底提示。
- 备选：给 `rfd` 打开 `gtk3` feature（需 `libgtk-3-dev` 与运行期 GTK3），或自绘保存框。

### 2.5 打印（唯一必须改写的功能）

现状链路：`paper::paint(c: &mut dyn Canvas, ..)`（毫米坐标版面）→ `gdi` 实现 `Canvas`（`line` / `rect` / `draw_text` / `measure_text`）→ ① 打印机 DC 出纸 ② 内存 DIB 转 RGBA 给界面预览。

Linux 上的实际行为：`render_print_page` 返回 `Err` → 预览窗**打不开**，弹出统一失败弹窗「打印功能仅支持 Windows」；`print_vouchers_now` 同样返回 `Err` → 点「打印」也是同一个弹窗。**优雅降级，不崩溃，但功能为零。**

**好消息**：`Canvas` trait 只有 4 个方法，是干净的移植缝。移植方案：

- **方案 P（推荐）**：新增 `pdf.rs` 实现 `Canvas`（`printpdf` 或 `pdf-writer`）→ 生成 A4 横向 PDF → `lp` / `gtk_print` 出纸；预览可用同一 PDF 经 `pdfium`/`mupdf` 光栅化，或直接用 egui 自绘。
- **方案 C**：新增 `cairo.rs` 实现 `Canvas` → Cairo PDF surface → CUPS。系统多一个 `libcairo` 依赖，但文字度量与 PDF 一致。
- **必须解决的差异**：GDI 用「宋体 / 新宋体」（`paper.rs` 的 `Face::Song` / `Face::NSong`）并以磅值换像素；Linux 无这两款字，需映射到「思源宋体 / Noto Serif CJK」并**嵌入 PDF**，否则字形替换会导致断行与版式漂移（这张表是要盖章上报的，版式漂移不可接受）。估计工作量 **2～4 人日**（含两页版面的像素级回归）。

### 2.6 其他已适配 / 待处理项

| 项 | 现状 | 处理 |
|---|---|---|
| 打开附件 | ✅ `#[cfg(not(windows))]` → `xdg-open` | 无 |
| 数据库位置 | `exe_dir()/data/ctp.db` | 装到 `/opt` 或 `/usr` 时**不可写**（`let _ =` 静默失败 → 建库失败提示「数据库未打开」）。需改为 `XDG_DATA_HOME/ctp/` 或安装到用户目录 |
| panic 日志 | `exe_dir()/data/panic.log` | 同上，写入失败会被 `let _ =` 吞掉 |
| 日期/时间 | 自实现东八区换算，**无 chrono / tzdata 依赖** | 无风险 |
| 开发用脚本 | `scripts/gui_probe.py` 用 Win32 API | 仅开发工具，不随发布 |
| 环境变量开关 | `CTP_THEME` / `CTP_SCREEN` / `CTP_SELFTEST` / `CTP_SHOT` 均为跨平台代码 | **可直接用于 Linux 无头冒烟**（见 8.3） |

---

## 3. 风险登记表

| ID | 风险 | 等级 | 触发条件 | 影响 | 缓解 |
|---|---|---|---|---|---|
| R1 | **ABI 1.0 与上游 Rust（新世界）不匹配** | 🔴 高 | 目标机 `readelf -h /bin/ls` 显示 ABI version 1 | 二进制无法启动或随机崩溃；交叉编译无法修复 | 换新世界镜像 / 用厂商 rustc / 见 9.1 决策树 |
| R2 | **glibc < 2.36** | 🔴 高 | `ldd --version` 为 2.28 | 链接或运行期版本符号缺失 | 目标机自建 std（nightly `-Zbuild-std`）/ musl 静态 / 厂商工具链 |
| R3 | 打印功能缺失 | 🔴 高（功能） | 必然 | 核心业务（付款单出纸）不可用 | 实现 `Canvas`→PDF + CUPS（2–4 人日） |
| R4 | OpenGL 能力不足 | 🟡 中 | 仅 LG100 驱动或驱动未装 | 启动黑屏 / panic（`glow` 初始化失败） | `LIBGL_ALWAYS_SOFTWARE=1`（llvmpipe）；或评估 `wgpu` 后端 |
| R5 | `xdg-desktop-portal` 缺失 | 🟡 中 | 精简版麒麟 | 导出保存框静默失败 | 安装 portal；或 `rfd` 开 `gtk3` feature；或给「取消」加区分提示 |
| R6 | CJK 字体缺失 / 字体名不匹配 | 🟡 中 | 最小化安装 | 界面方块；PDF 版式漂移 | 随包内置思源宋体 + 安装脚本声明 `fonts-noto-cjk` |
| R7 | 中文输入法在 X11 下不可用 | 🟡 中 | UKUI + fcitx5 + X11 | 无法录入中文 → 业务不可用 | 优先 Wayland 会话；实测 fcitx5；必要时升级 egui/winit |
| R8 | 数据库与日志写入只读目录 | 🟢 低 | 安装到 `/opt` | 首次启动建库失败 | 改用 XDG 目录；安装脚本 chown |
| R9 | 目标机缺 gcc / pkg-config | 🟢 低 | 最小化系统 | 无法原生构建 | 交叉编译，或装 `build-essential` |
| R10 | 编译耗时 / 内存 | 🟢 低 | 3A5000 单核弱 | 首次 release 约 20–40 分钟，峰值 2–3 GB | 复用 `sccache`；CI 缓存；`lto=false` |

---

## 4. 适配清单（按优先级）

### P0 — 不改则无法交付
1. **确认 ABI 与 glibc**，据此选工具链（第 8 节第 0 关）。
2. **打印输出通道**：新增 `crates/ctp-app/src/pdf.rs`（或 `cairo.rs`）实现 `paper::Canvas`；`lib.rs` 的 `#[cfg(not(windows))]` 分支改为调用新后端；用 CUPS（`lp -o media=A4 -o landscape` 或 `gtk-print` 对话框）出纸。
3. **PDF 字体嵌入**：`Face::Song/NSong` → 思源宋体 / Noto Serif CJK，**必须嵌入子集**。

### P1 — 能跑但功能/体验缺陷
4. `rfd` 保存框后端：确保 `xdg-desktop-portal` 在跑，否则启用 `gtk3` feature；把「取消」与「后端不可用」区分开（当前都静默）。
5. 数据库与 panic 日志迁移到 `$XDG_DATA_HOME/ctp/`（保留「exe 同级 data/」作为兼容回退）。
6. 字体：随包携带思源宋体（`assets/fonts/`），`fonts.rs` 候选表加入 `include_bytes!` 兜底。
7. 打包与桌面集成：`.desktop` 文件、图标、`deb` 包（`cargo-deb`）或 AppImage。

### P2 — 优化
8. `[profile.release]` 增加 `strip = true`、`lto = "thin"`，减小体积（当前 23 MB）。
9. 大表格滚动性能（egui 逐行 `ui.label` 在弱 CPU 上可能掉帧，可改 `ScrollArea::show_rows`）。
10. 打印预览改成 PDF 光栅化后复用（避免两套渲染导致预览与出纸不一致）。

---

## 5. 原生编译路径（目标机上直接编）

**前置包**（Debian 系命名，麒麟对应同名或近似）：

```bash
sudo apt install -y build-essential pkg-config git curl
sudo apt install -y libxkbcommon-dev libxkbcommon-x11-dev \
                    libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev \
                    libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
                    libgl1-mesa-dev libegl1-mesa-dev
sudo apt install -y fonts-noto-cjk xdg-desktop-portal cups-client
# 可选：软件渲染兜底
sudo apt install -y mesa-utils libgl1-mesa-dri
```

**工具链**：优先 `rustup`（若提供 loongarch64 主机工具链）；否则用发行版仓库/龙芯社区提供的 `rustc`+`cargo`。

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
rustup target list --installed        # 确认含 loongarch64-unknown-linux-gnu
rustc -vV                             # 记录 host 三元组与 LLVM 版本

cd /path/to/pay2
cargo build --release --locked -p ctp-gui     # 首次含 bundled SQLite，约 20–40 分钟
cargo test  --workspace --locked              # 期望 ~149 passed / 1 ignored（3 项 Windows 专属自动剔除）
```

**判定**：`cargo test` 全绿即证明 std / libc / SQLite / 业务层在该 ABI 上自洽；GUI 另按第 8 节第 3 关验证。

---

## 6. 交叉编译路径（x86_64 构建机 → loongarch64）

### 6.1 工具链选择（决定成败，不是可选项）

| 目标系统 | 需要的 C 工具链 | 与上游 Rust std 的关系 |
|---|---|---|
| 新世界 / ABI 2.0 / glibc ≥ 2.36 | Loongson 官方 build-tools 的 `loongarch64-linux-gnu-gcc`（GCC 13） | ✅ 一致，直接可用 |
| 旧世界 / ABI 1.0 / glibc 2.28 | Loongnix 的 GCC 12（`-mabi=lp64d` 旧约定） | ❌ 与 std 不一致，链接能过、运行可能崩 |

### 6.2 配置（新建 `.cargo/config.toml`）

```toml
[target.loongarch64-unknown-linux-gnu]
linker = "loongarch64-linux-gnu-gcc"
# 运行期在可执行文件同级 lib/ 找私有 .so（如需）
rustflags = ["-C", "link-arg=-Wl,-rpath,$ORIGIN/lib"]

[env]
CC_loongarch64_unknown_linux_gnu = "loongarch64-linux-gnu-gcc"
AR_loongarch64_unknown_linux_gnu = "loongarch64-linux-gnu-ar"
PKG_CONFIG_ALLOW_CROSS = "1"
# 若为目标 sysroot 里的 X11/Wayland .pc：
# PKG_CONFIG_SYSROOT_DIR = "/opt/loongarch64-sysroot"
# PKG_CONFIG_PATH = "/opt/loongarch64-sysroot/usr/lib/pkgconfig"
```

```bash
rustup target add loongarch64-unknown-linux-gnu
cargo build --release --locked -p ctp-gui --target loongarch64-unknown-linux-gnu
```

**必须注意**：
- `libsqlite3-sys` 会调 `cc`，`CC_loongarch64_unknown_linux_gnu` 不设就会用主机 gcc 产出错误架构的目标文件。
- GUI 的 X11/GL 依赖多数在**运行期 dlopen**，编译期主要卡在 `pkg-config` 找 `xkbcommon`/`wayland-client`；没有 sysroot 时最简单的做法是容器内交叉（镜像自带 `-dev` 包）。
- **交叉不会改变 ABI**（见 2.2），别指望「用旧世界 gcc 链接就能跑在旧世界」。

### 6.3 产物自检（不需要目标机）

```bash
BIN=target/loongarch64-unknown-linux-gnu/release/ctp-gui
file $BIN                                            # 应为 ELF 64-bit LSB, LoongArch
readelf -h $BIN | grep -i flags                      # 看 e_flags 的 ABI version
objdump -T $BIN | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -3   # 最高 glibc 符号版本
```

把 `readelf` 的 ABI version 与目标机 `/bin/ls` 的对比，**一致**才继续；`objdump` 得到的最高 GLIBC 版本必须 ≤ 目标机 `ldd --version`。

---

## 7. GitHub Actions 自动化编译

### 7.1 现状（已有基础，改造成本低）

`.github/workflows/build.yml` 已经是三段式（test / build matrix / release），且**曾经支持过 Linux 构建**，注释写得很清楚：

```
# 【临时调整】Linux 产物暂停构建，只保留 Windows 版；matrix 结构刻意保留，
# 恢复时只需把下面 include 里的 ubuntu-latest 一项加回来……
```

`test` job 已经在 `ubuntu-latest` 上装齐 X11/Wayland/GL 依赖跑 `fmt` / `clippy` / `test`——**这说明 GUI 在 Linux x86_64 上是编译通过的**，是本报告最有力的正面证据之一。

### 7.2 方案 A：真交叉（推荐，速度快）

GitHub **不提供 loongarch64 runner**，但交叉编译只需要 x86 上能跑的交叉 gcc：

```yaml
  build-loongarch64:
    name: build (ctp-linux-loongarch64)
    needs: test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<pin-sha>
      - name: 安装 LoongArch 交叉工具链
        run: |
          curl -fsSL -o /tmp/tc.tar.gz <loongson-build-tools-url>/loongarch64-linux-gnu-gcc13.tar.gz
          sudo mkdir -p /opt/la64 && sudo tar -xzf /tmp/tc.tar.gz -C /opt/la64 --strip-components=1
          echo "/opt/la64/bin" >> $GITHUB_PATH
      - name: 安装 GUI 依赖（sysroot 或宿主 .pc）
        run: sudo apt-get update && sudo apt-get install -y pkg-config libxkbcommon-dev libwayland-dev libx11-dev libgl1-mesa-dev
      - uses: Swatinem/rust-cache@<pin-sha>
        with: { key: loongarch64 }
      - run: rustup target add loongarch64-unknown-linux-gnu
      - run: cargo build --release --locked -p ctp-gui --target loongarch64-unknown-linux-gnu
      - name: 自检 + 打包
        run: |
          BIN=target/loongarch64-unknown-linux-gnu/release/ctp-gui
          readelf -h $BIN | grep -i flags
          objdump -T $BIN | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -3
          tar czf ctp-gui-linux-loongarch64.tar.gz -C $(dirname $BIN) ctp-gui
      - uses: actions/upload-artifact@<pin-sha>
        with: { name: ctp-linux-loongarch64, path: ctp-gui-linux-loongarch64.tar.gz }
```

优点：跑在原生 x86 runner 上，10–20 分钟出包；缺点：**只能产出新世界（ABI2.0）二进制**，且不验证运行。

### 7.3 方案 B：QEMU 容器内原生构建（能跑测试）

```yaml
      - uses: docker/setup-qemu-action@<pin-sha>
        with: { platforms: loong64 }
      - run: |
          docker run --rm --platform linux/loong64 -v $PWD:/src -w /src \
            <loong64-image-with-rust-and-qt/gtk-deps> \
            bash -lc "cargo test --workspace --locked && cargo build --release -p ctp-gui"
```

- 适合：**验证 ABI/glibc 真实性**、跑 `cargo test`（这是最有价值的自动化）。
- 代价：QEMU 用户态模拟下编译慢 3–8 倍（`sqlite3.c` 尤其慢），建议只在 `workflow_dispatch` 或夜间跑，缓存 `target/`。
- 镜像：优先龙芯官方 `lcr.loongnix.cn` / `ghcr.io/loongson/*` 的 loong64 基础镜像（自带 gcc/glibc 与 Rust 的可能性需确认）。

### 7.4 方案 C：自托管 runner（最稳，长期）

在龙芯机器或龙芯云上注册 self-hosted runner，直接 `cargo build` + `cargo test` + 冒烟截图（配合 `xvfb-run` 与项目已有的 `CTP_SHOT`）。唯一能覆盖「GUI 真能起来」的方案。

> 三种方案不互斥：**A 出包、B 跑测试、C 做验收**。

---

## 8. 可直接执行的验证步骤

### 第 0 关：目标机事实采集（5 分钟，决定后续一切）

```bash
uname -m                                  # 期望 loongarch64
cat /etc/os-release                       # 发行版与版本（Kylin V10 SPx）
ldd --version | head -1                   # glibc 版本：≥2.36 才谈上游 Rust
readelf -h /bin/ls | grep -i flags        # e_flags：ABI version 1 还是 2
getconf GNU_LIBC_VERSION
ls /usr/share/fonts/**/NotoSansCJK* 2>/dev/null | head    # CJK 字体
echo $XDG_SESSION_TYPE; glxinfo -B 2>/dev/null | head -6  # X11/Wayland + GL 版本
systemctl --user status xdg-desktop-portal 2>/dev/null | head -3
```

| 采集结果 | 判定 |
|---|---|
| ABI version 2 且 glibc ≥ 2.36 | ✅ 走上游工具链，源码基本零改动（打印除外） |
| ABI version 1 **或** glibc < 2.36 | ⚠️ 走上游工具链大概率失败，转第 9 节绕行方案 |
| 无 Noto CJK | 装 `fonts-noto-cjk`，否则界面方块 |
| GL < 3.3 | 先试 `LIBGL_ALWAYS_SOFTWARE=1 ctp-gui` |

### 第 1 关：编译（证明工具链可用）

```bash
cargo build --workspace --locked          # 只编业务层，先不碰 GUI
cargo build --release -p ctp-gui          # 再编 GUI
```
判定：无 `GLIBC_2.xx not found`、无 `cannot find -lX11` 类错误。

### 第 2 关：测试（证明 ABI 自洽，最有信息量）

```bash
cargo test --workspace --locked
```
判定：约 **149 passed / 1 ignored / 0 failed**（`tests/print.rs` 内 3 处 `#[cfg(windows)]` 的渲染用例在 Linux 上自动剔除）。若出现 `SIGSEGV` / `Illegal instruction`，基本可判定为 R1（ABI 不匹配），直接转第 9 节。

### 第 3 关：GUI 冒烟（项目已内置无头能力）

代码里已有 `CTP_SELFTEST=1`（跑若干帧自动退出）与 `CTP_SHOT=<out.ppm>`（渲染一帧存盘），**正好用于 Linux 无显示器环境**：

```bash
xvfb-run -s "-screen 0 1600x1000x24" \
  env CTP_SELFTEST=1 CTP_ROLE=admin CTP_SCREEN=contracts \
  ./ctp-gui
# 或留证据：
xvfb-run -s "-screen 0 1600x1000x24" env CTP_SHOT=/tmp/shot.ppm CTP_ROLE=admin ./ctp-gui
```
判定：进程 0 退出、`data/ctp.db` 生成、截图里中文不是方块、表格有数据。若黑屏 → 加 `LIBGL_ALWAYS_SOFTWARE=1` 重试（只重试一次，按项目既有纪律）。

### 第 4 关：打印（当前预期失败，改造后复验）

```bash
# 现状预期：点「打印」→ 弹「打印功能仅支持 Windows」
./ctp-gui
# 改造后：
lpstat -p -d                            # 有 CUPS 打印机
# 生成的 PDF 应满足：2 页 A4 横向、297×210mm、字体已嵌入
pdffonts out.pdf                        # 应显示 NotoSerifCJK 且 emb=yes
```

---

## 9. 最终建议

### 9.1 决策树

```
第 0 关结果
├─ ABI 2.0 且 glibc ≥ 2.36
│   └─ ✅ 直接原生编译（或 GitHub Actions 交叉出包）
│      源码改动仅：打印后端 + DB 路径 + 字体兜底 ≈ 3–5 人日
├─ ABI 2.0 但 glibc < 2.36
│   ├─ 目标机可装 nightly → cargo +nightly -Zbuild-std（用系统 glibc 重编 std）
│   ├─ 或换 loongarch64-unknown-linux-musl 静态链接（需 musl 交叉 gcc；GUI 仍动态依赖 X11/GL）
│   └─ 或升级系统到 glibc ≥ 2.36（最省事，需甲方许可）
└─ ABI 1.0（旧世界）
    ├─ ① 用麒麟/龙芯官方提供的旧世界 rustc（若存在）——先问厂商，这是唯一低风险路径
    ├─ ② 申请换新世界镜像（V10 SP3 新世界版）——工程上最划算
    └─ ③ 放弃 Rust 原生 GUI，改「Windows 出数 + 国产化环境只做查看」的降级交付
```

### 9.2 分阶段建议

| 阶段 | 目标 | 产出 | 预估 |
|---|---|---|---|
| S0 | 事实采集 | 第 0 关输出表 | 0.5 小时 |
| S1 | 跑通非打印功能 | `cargo test` 全绿 + GUI 冒烟截图 | 1–2 人日（若 ABI 匹配） |
| S2 | 打印适配 | `pdf.rs` + CUPS + 字体嵌入 + 版面回归 | 2–4 人日 |
| S3 | 交付工程化 | `.deb` / AppImage、XDG 路径、桌面文件、安装说明 | 1–2 人日 |
| S4 | CI | 恢复 Linux matrix + 新增 loongarch64 交叉 job + 夜间 QEMU 测试 | 0.5–1 人日 |

### 9.3 三条明确判断

1. **不要先改代码**：在 ABI/glibc 未确认前写任何适配都可能白做——业务层没有任何需要为龙芯改的地方。
2. **打印是唯一的真活**，且价值最高：`Canvas` trait 已把成本压到「实现 4 个方法 + 字体映射」，不要重写版面。
3. **CI 里那条被注释掉的 Linux matrix 就是最好的起点**：先恢复 `ubuntu-latest`（x86_64）保证 Linux 路径不腐化，再加 loongarch64 交叉 job，最后按需补 QEMU 测试。

---

## 10. 假设与不确定项（结论的边界）

| 编号 | 假设 / 不确定项 | 若不成立的影响 |
|---|---|---|
| A1 | 目标系统为**麒麟 V10 早期版本，glibc 2.28**（用户只说 ABI 1.0，未给 glibc） | 若实际 glibc ≥ 2.36，R2 风险直接消失，方案大幅简化 |
| A2 | 「ABI 1.0」= 旧世界（Loongnix 20 / 早期 3A5000 生态），非新世界 | 若实为新世界，「ABI 1.0」可能只是笔误，R1 消失 |
| A3 | 目标机运行 X11 会话（UKUI 默认），有可用 GL 或可退回 llvmpipe | 纯 Wayland + 无 GL 时需改后端 |
| A4 | 目标机可访问内网/外网软件源以安装 `-dev` 包（或已有） | 离线环境需预置 sysroot 与 .deb 依赖包 |
| A5 | **未实测**：本机无 loongarch64 硬件、无 LoongArch 交叉 gcc。关于 glibc 2.36 的结论来自 `libstd.so` 的 `.gnu.version_r` 静态提取，**属于证据+推断，不是运行验证** | 若静态链接实际不要求 2.36，则 R2 可降级 |
| A6 | SQLite 在 LoongArch 上无汇编优化，性能略降但功能正确 | 大量数据（>10 万行）时报表变慢，可接受 |
| A7 | egui/glow 在 llvmpipe 下可启动（帧率低） | 若无法启动，需评估 `wgpu`/Vulkan 或改用 GTK 原生控件重写界面（成本极高） |
| A8 | 中文 IME 在 fcitx5 + X11 下可用 | 若不可用，需切 Wayland 会话或升级 egui/winit |
| A9 | GitHub 不提供 loongarch64 hosted runner（当前事实），CI 必须靠交叉或 QEMU/自托管 | 若未来提供，方案 A/B 可简化 |
| A10 | 龙芯官方 `build-tools` 与 `lcr.loongnix.cn` 镜像可用性未验证（无网络实测） | 需改用 crosstool-ng 自建或目标机原生构建 |

---

## 附：一页速查

```
能编吗？      能。Rust 4 crate 全纯 Rust（除 bundled SQLite 需 gcc）
能跑吗？      取决于 ABI/glibc：ABI2.0+glibc2.36 → 是；ABI1.0/glibc2.28 → 否（需绕行）
要改什么？    ① 打印：Canvas→PDF+CUPS（唯一真活） ② 字体兜底 ③ DB 路径 XDG ④ rfd 后端
谁写代码？    gdi.rs 的替代者（新增 pdf.rs），业务代码零改动
CI 怎么做？   恢复被注释的 Linux matrix → 加 loongarch64 交叉 job → 夜间 QEMU 跑 cargo test
先做哪一步？  第 0 关 5 分钟事实采集（ldd --version / readelf -h /bin/ls）
```
