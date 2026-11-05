# 工程合同台账与拨款管理平台（本期实现）

依据《工程合同台账与拨款管理平台_软件开发文档_v1.6》与其评审意见
（`工程合同台账与拨款管理平台_软件开发文档_v1.6_评审意见.md`）落地的 Rust 实现。

**本期范围**：合同台账 + 拨款管理 + 四个报表视图 + 口径自检。
**不含**：支付申请、三级审批、红冲、结算、造价指标、RBAC —— 均为评审确认的延期项（见 `docs/口径主表.md` C 节）。

---

## 一、工程结构

```
D:\pay2
├─ Cargo.toml                 工作区
├─ crates/
│  ├─ ctp-core/               领域层（无外部依赖，口径都在这里）
│  │  ├─ money.rs             金额（分，i64）+ 宽容解析 + 财务括号显示 + 打印用固定两位
│  │  ├─ amount_cn.rs         人民币金额转中文大写（零壹贰…圆角分整）
│  │  ├─ rate.rs              比率（万分比）+ Rate::ratio（H1 的唯一实现）
│  │  ├─ contract.rs          合同实体、状态机、有效计价基准、业务字段清单（M1）
│  │  ├─ payment.rs           拨款单实体、PAID/VOID 两态（H5）
│  │  ├─ quota.rs             可付额度、容差、越界级别（BR-01 / BR-15）
│  │  ├─ attachment.rs        四类合同附件（中标通知书/合同/中期结算/审计报告）、50 MB 上限
│  │  ├─ digest.rs            SHA-256 附件指纹（无外部依赖）
│  │  ├─ dict.rs              合同类别 & 会计分类两套字典（H4）
│  │  ├─ date.rs / ids.rs     日期校验、编号生成
│  │  ├─ refs.rs              RV-/RK-/RPT- 前缀规范（M6）
│  │  └─ scope.rs             本期范围台账 + P0 口径校验（M5）
│  ├─ ctp-store/              SQLite
│  │  ├─ schema.rs            建表/索引/视图；已付子查询与比率表达式唯一实现（H1/H2/M3/M4）
│  │  ├─ db.rs                连接、PRAGMA、迁移（含单位角色表重建）、流水号、元数据
│  │  ├─ repo.rs              仓储与视图映射；附件读写；DB-06 结构自检
│  │  └─ seed.rs              基础字典
│  ├─ ctp-app/                应用服务（事务、留痕、额度校验）
│  │  ├─ service.rs           合同登记/生效/作废、金额编辑、审计价、拨款、附件、档案自动建档、报表导出
│  │  ├─ report.rs            ReportResult：屏幕表格 / Excel 导出 / 后期打印共用的唯一数据源
│  │  ├─ xlsx.rs              rust_xlsxwriter 写出（表头 / 冻结 / 合计公式 / #,##0.00）
│  │  ├─ print.rs             9 域描述符与取值解析 → 折成版面字段（Voucher）
│  │  ├─ paper.rs             设备无关版面层：两页横向 A4 的坐标与绘制（出纸与预览共用）
│  │  ├─ gdi.rs               打印后端（仅 Windows）：PrintDlgW + StartDocW 出纸 / 内存 DIB 预览
│  │  ├─ checks.rs            六项口径自检（界面「运行自检」的唯一实现）
│  │  ├─ history.rs           金额与审计价变更历史（JSON，保留 50 条）
│  │  └─ tests/consistency.rs 21 项集成断言（评审条目逐条对应）
│  └─ ctp-gui/                egui 桌面界面：六个页面 + 六类弹窗，浅色 / 深色双主题
├─ scripts/                   WSL 构建 / 端到端演练 / 界面截图核验
└─ docs/口径主表.md           金额与比率的单一事实来源
```

技术栈：Rust 1.99（edition 2021）+ rusqlite 0.32（bundled，自带 SQLite，无系统依赖）
+ rust_xlsxwriter 0.102（报表导出）+ windows-sys 0.59 / raw-window-handle 0.6（打印，仅 Windows）。
无 WebView、无前端框架、无 Node，打印也不依赖任何办公软件 —— 与文档 ADR-06 的技术前提一致。

> **注意**：`rust_xlsxwriter` 是 **write-only**，只能新建 .xlsx，**不能读取或修改已有的 Excel
> 模板**。因此「读取 Excel 模板 → 填充占位符」不可行。打印件则完全不走模板文件：
> 版面写在 `paper.rs` 里、用 GDI 直接画到打印机，见下文「付款打印」与文档 R-28。

**交付形态只有 GUI**：工作区唯一的 bin 是 `ctp-gui`，命令行版本已按要求清理，
不提供也不计划补 CLI。

---

## 二、在 WSL 下编译与运行

工具链已在 WSL Debian 内就绪（`cargo 1.99.1` / `gcc 14.2`）。源码在 `/mnt/d/pay2`，
**target 目录放在 WSL 原生盘**（`$HOME/target-ctp`），避开 9p 挂载的编译慢速问题。

```bash
# 一次性：加入 PATH 并设置 target 目录
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/target-ctp"
cd /mnt/d/pay2

cargo build --workspace          # 编译（首次会编译 bundled SQLite，约 1 分钟）
cargo test  --workspace          # 96 项测试
```

或直接跑现成脚本：

```bash
bash scripts/build_all.sh        # 编译 + 全部测试
bash scripts/e2e.sh              # 端到端演练，结果写 build-log.txt
```

### 运行

```bash
cargo run -p ctp-gui        # 启动图形界面
```

首次启动自动建库并写入基础字典，数据库落在可执行文件同级的 `data/ctp.db`
（开发时即 `target/debug/data/ctp.db`）。总览页的「载入演示数据」可一次生成
2 个项目、3 份合同、5 笔拨款，用来演示四个报表的口径。

界面六个页面：总览、合同台账、拨款管理、分类汇总、期间统计、自检与设置。
自检页的六项与 `ctp_app::checks` 同源：视图比率与应用层一致、DB-06 审计列齐全、
P0 全在本期、外键无悬空、已付口径与「仅 PAID 合计」一致、视图无丢行。

### 新建合同的录入方式

分两步：**基本信息 → 附件**。

- **所属项目 / 承包单位 / 发包人（甲方）** 都是可自由输入的联想框：输入任意片段即浮出
  候选（项目按「编码 + 名称」双匹配），点选即回填；也可以直接输入库里没有的新名称。
- 名称在档案里不存在时**自动建档**：项目编码按 `PRJ-年份-三位流水` 生成；单位按合同类型
  映射角色（施工 / 监理 / 设计 / 勘察 / 供应商），发包人固定记为「业主」。同名复用，不重复建。
- **会计分类**已从表单移除（数据库列保留，新建写空，历史数据的分类汇总不受影响）。
- **四类附件**（中标通知书 / 合同 / 中期结算材料 / 审计报告）分项存储，每类一份，
  重新选择即替换。文件本体以 BLOB 存进 `ct_contract_attachment`，单份上限 50 MB，
  因此整套系统始终只有一个 `ctp.db`，备份不会漏掉附件。
- **合同为正本必传项**，但不是保存时强卡：没有合同附件仍可存为草稿，卡点在「设为生效」。
  这样手里暂时没有电子件时也能先把合同信息录进来。
- 保存后的附件在合同详情卡的「附件 n/4」里管理：打开（落地临时文件后调系统默认程序）、
  另存、替换、删除。

### 报表导出（`rust_xlsxwriter`）

「分类汇总」（V3）与「期间统计」（V4）两页标题右侧各有「导出 Excel」按钮：
弹出保存框 → 生成单页 .xlsx → 写一条 `EXPORT` 审计日志。

- **屏幕表格、柱状图、导出共用同一份 `ReportResult`**（`ctp-app/src/report.rs`），
  列配置驱动，不再各自查库、各自算合计；
- 版式：标题行 + 口径说明行 + 表头行（冻结前三行）+ 数据行 + 合计行；
  金额列写**数值**并套 `#,##0.00`（单位「元」，库内存分），合计用 `SUM` 公式，
  便于在 Excel 里二次筛选计算；页脚注明导出人 / 导出时间 / 系统版本；
- 文件名默认 `{报表名}_{yyyyMMdd_HHmm}.xlsx`。

### 付款打印（不依赖任何外部软件）

打印由程序自己排版、自己画到打印机：**不经过 Word / Edge / PDF 阅读器**，也不需要本机装任何办公软件。

- 入口三处，共用一套逻辑：总览「最近拨款」行内文字链、合同台账展开明细行内、拨款管理明细行内；
- 点开后是**核对窗**：左侧真实位图预览（可翻页），右侧 9 个域的取值与来源；
  点「打印」直接弹 Windows 系统打印对话框出纸；
- 版面：两页横向 A4 —— 第 1 页「财务支出审核监督表」、第 2 页「工程项目进度款支付申请表」，
  坐标从实物模板 PDF 逐线量取（`paper.rs` 的 `S1_ROWS` / `S2_COLS` 等常量）；
- 批量：仅拨款单，明细里勾选若干条 → 「批量打印所选」，合并成一份、**每张两页**；
- 作废单不允许打印（入口不出现 + 生成前校验双保险），也不可勾选；
- **纸型必须 A4 横向**：A4 纵向会裁掉右半边，程序取到打印机后立刻拦下并提示改纸型，不静默打半张；
- 字段映射（口径与模板一致）：

  | 域 | 取值来源 | 落在版面哪里 |
  |---|---|---|
  | 项目名称 | **合同名称**（不是项目名称） | 第 1 页支付用途说明 + 第 2 页项目名称 |
  | 中标单位 | **承包单位** | 第 2 页施工单位 |
  | 合同改成元 | 合同价（元、两位小数） | 第 2 页合同价 |
  | 审计价改成元 | 审计价；**未审定填 0**（输出 `0.00`） | 第 2 页审计价 |
  | 已付款改成元 | **本次之前**的累计已付（按开单日期、排除作废） | 第 2 页目前已累计支付金额 |
  | 本次付款小写 | 本次金额 | 第 1 页小写 + 第 2 页本期申请支付金额 |
  | 本次付款数大写 | 本次金额 → 人民币中文大写 | 第 1 页大写 |
  | 本次付款后累计 | 已付 + 本次 | 第 2 页本期末累计支付金额 |
  | 合同约定_付款条件 | 付款条款 | 第 2 页合同履行情况 |

- **留空**：会计分类名、附件张数、当前日期三处保持空白供手工填写 / 签字；
  其他字段缺失时也一律留空，不写「—」；
- 实现分两层：
  - `ctp-app/src/paper.rs` —— 设备无关版面层。`Canvas` trait（`line` / `rect` / `draw_text` /
    `measure_text`，mm 坐标）是唯一绘图接口，`paint()` 是唯一排版入口。**出纸与预览共用这一份代码**，
    所以预览图就是成品；
  - `ctp-app/src/gdi.rs`（`#[cfg(windows)]`）—— 两个后端。`print_vouchers` 走
    `PrintDlgW`（弹系统打印对话框取 DC）+ `StartDocW` / `StartPage` / `EndPage` / `EndDoc`（多页作业）；
    `render_page` 走 `CreateDIBSection` 渲染到内存供界面预览。两者只差坐标缩放。
- **工程坑（`windows-sys 0.59`）**：`StartDocW` / `StartPage` / `EndPage` / `EndDoc` / `DOCINFOW`
  定义在 `Win32::Storage::Xps`（不是 `Win32::Graphics::Printing`），要开 `Win32_Storage_Xps` feature；
  `RECT` 在 `Win32::Foundation`。`CreateFontW` 用负高度表示字符高度，pt→px 为 `dpi × 25.4 / 72`。
  内存 DIB 的 BGRA→RGBA 必须在 `DeleteObject(dib)` **之前**拷完，否则读已释放内存会直接崩
  （`STATUS_ACCESS_VIOLATION`）。
- 打印附件另走一条：按原件交给系统默认程序打开，由用户自己选打印范围，程序不接管、不合并、不批量。

窗口可自由缩放，最小 1000×640；宽度小于 1100px 时左侧导航折叠为缩写并带悬停提示。
右上角可切换浅色 / 深色。

**发布版不带控制台窗口**：`main.rs` 在非 debug 构建里声明了
`windows_subsystem = "windows"`，双击即直接进界面；崩溃信息写到同级的
`data/panic.log`，启动失败写 `data/startup-error.log`（GUI 模式看不到 stderr）。

### 界面自动核验

界面没有控制台输出，因此内置几个环境变量开关（正常启动不生效）：

```powershell
$env:CTP_SELFTEST=1; .\ctp-gui.exe        # 跑 40 帧后退出，写 data/smoke.txt（页名 / 合同数 / 拨款数）
$env:CTP_SHOT="out.ppm"; .\ctp-gui.exe    # 等界面稳定后由渲染器保存一帧，退出码 0
$env:CTP_SCREEN="payments"; $env:CTP_THEME="dark"   # 指定启动页与主题
$env:CTP_DIALOG="new_contract"             # 启动即打开弹窗；另有 attachments / payment_detail / print_preview / print_batch / material_print / audit_price / edit_amount / void_payment / contract_detail
$env:CTP_DIALOG="new_contract"; $env:CTP_STEP="1"   # 直达新建合同的第二步（附件）
$env:CTP_DIALOG="print_preview"            # 启动即打开打印核对窗（真实位图预览 + 九域取值）
```

`scripts/gui_probe.py` 在此之上提供截图与点击回放。打印窗口（PrintWindow）对
GPU 呈现的窗口常返回空白，可加 `grab=screen` 改走屏幕直抓。

### 界面预览

| 总览 | 合同台账 |
|---|---|
| ![总览](shots/01-总览.png) | ![合同台账](shots/02-合同台账.png) |

| 拨款管理 | 期间统计 |
|---|---|
| ![拨款管理](shots/03-拨款管理.png) | ![期间统计](shots/04-期间统计.png) |

| 深色主题 | 新建合同弹窗 |
|---|---|
| ![深色主题](shots/05-深色主题.png) | ![新建合同](shots/06-新建合同.png) |

---

## 三、评审意见 → 代码落点

| 评审 | 问题 | 代码落点 |
|---|---|---|
| **H1** | V1 比率写成「除后乘 2」，与 V3/Rust 不一致 | `schema::rate_bp_expr` 把 `*2` 放回分母；`Rate::ratio` 为唯一实现；`selfcheck` 与集成测试断言差异为 0 |
| **H2** | 已付取数源三处互斥（分摊表 / 直连 / 混合） | 只保留 `schema::paid_net_subquery` 一处，视 V1～V4 与仓储共用；分摊表本期不建 |
| **H3** | 用例与流程含已移出的申请/审批 | 无 `pay_application` 系列表；`register_payment` 单级登记；合同状态机去掉审批分支 |
| **H4** | `ct_category` 一名两义（合同类别 / 会计分类） | 拆两套字典：合同类别 `sys_dict_item`，会计分类 `ct_category`；`Contract` 同时持两个字段；测试断言编码不重叠。界面已不录入会计分类（列保留、新建写空），历史分类汇总照常可查 |
| **H5** | 红冲「未实现」却又被当作必要条件 | 彻底移除红冲：`status ∈ {PAID, VOID}`、无 `reversal_of`；作废即不计入已付，等价且少一状态机 |
| **M1** | 合同字段数 18/19 不一，`payment_terms` 缺失 | `BUSINESS_FIELDS` 单一清单（25 项，含 `payment_terms`），`business_field_count()` 为唯一计数 |
| **M2** | 已移出模块的残留 | 代码只实现本期模块；`scope::PLAN` 显式登记延期项 |
| **M3** | 建了被否决的索引、`application_id` 悬空外键 | 不建 `ix_contract_audited`；无 `application_id` 列 |
| **M4** | DB-06 要求每表审计列，实际多处缺失 | 业务表全部补齐 5 列，`sys_audit_log` 列入显式豁免；`repo::audit_column_violations` 可执行校验 |
| **M5** | P0 项却在白名单、E-16 优先级两处冲突 | `scope::PLAN` 台账 + `audit_p0_consistency()`：P0 必须在本期，白名单项一律 P1/P2 |
| **M6** | 三套 `R-` 前缀串号 | `refs::RefKind`：只允许 `RV-` / `RK-` / `RPT-`，`R-` 解析直接拒绝 |
| **L1** | 样例数据自相矛盾 | 演示数据只有一套基准（2,850,000 / 118,000 / 500,000），台账、分类、期间三表可互相验算 |
| **L2–L4** | 修订记录顺序、目录写法、视觉色板 | 属文档层面问题，代码不涉及；目录约定已在本文第二节固定为单一写法 |

---

## 四、与文档原稿的有意偏离（均为评审结论）

1. **本期建表 13 张**，而非目标态 21 张：只建被实际读写的表，避免空表带来维护成本。
   （第 13 张是 `ct_contract_attachment`，本期新增的合同附件表。）
2. **`pay_payment.amount_cent > 0`**：作废替代负数红冲，故拨款单恒为正。
3. **合同状态保留 8 个枚举值**以便库内往返，但只有 5 个可流转（`ContractStatus::PHASE_STATUSES`）。
4. **合同编号**沿用「当前实现」口径 `HT-{年}-{三位流水}`，目标态四位流水属二期（4.2.4 已注明需迁移）。

### R-25 三条决策（2026-11-05，正文需求描述未改）

1. **只保留 GUI**：工作区唯一 bin 为 `ctp-gui`，不做、也不再补命令行版本；
2. **去掉「允许未分类暂存于草稿」**：`ct_contract_item.category_id` 保持 `NOT NULL`，
   不重建表、不加迁移；`ct_contract.category_id` 也保持现状（新建合同不录入会计分类，
   写空值），不改表结构；
3. **去掉清单明细**：`ct_contract_item` 不做录入界面 / 命令 / 批量导入 / BR-05 闭合校验。
   **空表与 V3 视图里「按清单行分摊」的分支都保留**（删它们要重建视图、动迁移，收益为零），
   标注为「分支保留但本期无录入入口，恒不命中；未审定合同走 R-20 主导分类整笔归集」。

---

## 五、下一步建议（按投入产出）

1. 合同台账 / 拨款一览两页也接 `ReportResult`，让全部报表统一走列配置驱动；
2. 打印件如需再调整版式，改 `paper.rs` 的版面常量即可（改版需重新发版）；
   若要接第二台非 Windows 打印机或留 PDF，在 `gdi.rs` 旁加一个同 `Canvas` 语义的后端即可，
   `paper::paint` 不用动；
3. 二期恢复申请与审批时，只需加 4 张预留表并把 `inflight` 从 0 改为两类占用，`PaymentQuota` 无需改动。
