//! 合同台账页：筛选、列表、右侧详情与操作入口。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::service;
use ctp_app::{LedgerColumns, ReportResult};
use ctp_core::contract::ContractStatus;
use ctp_core::money::Money;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

/// R-64：台账可排序的四个字段。排序状态是**单一值**（字段 + 方向），换字段即覆盖旧字段，
/// 所以不可能出现「两个字段同时生效」或「旧列箭头没清掉」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LedgerSortKey {
    /// 合同金额
    Amount,
    /// 审计价（未审计 = 无值）
    Audit,
    /// 累计已付
    Paid,
    /// 支付率
    Rate,
}

impl LedgerSortKey {
    /// 列 key（`ReportResult` 的 `ReportColumn.key`）→ 排序字段。
    /// 只有这四列可排，其余（编号 / 名称 / 承包单位 / 交工日期 / 项目状态）返回 None，表头不可点。
    pub fn of_column(key: &str) -> Option<Self> {
        match key {
            "total_amount" => Some(Self::Amount),
            "audit_price" => Some(Self::Audit),
            "paid_amount" => Some(Self::Paid),
            "pay_rate" => Some(Self::Rate),
            _ => None,
        }
    }
}

/// R-65：表头的三种身份。除四个可排序列外，「合同编号」是**恢复初始排序**的入口
/// （初始排序 = 库返回的原始行序，即不排序），其余列是纯文本。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HeadKind {
    Sort(LedgerSortKey),
    ResetSort,
    Plain,
}

fn head_kind(key: &str) -> HeadKind {
    if key == "contract_no" {
        return HeadKind::ResetSort;
    }
    match LedgerSortKey::of_column(key) {
        Some(k) => HeadKind::Sort(k),
        None => HeadKind::Plain,
    }
}

/// 表头一格的呈现：标题文字、是否高亮、悬停提示。三处表头共用它，
/// 箭头（可排序列）与 ↺（编号列）就不会各写一套而互相不一致（R-64 / R-65）。
struct HeadView {
    label: String,
    active: bool,
    tip: String,
}

fn head_view(title: &str, kind: HeadKind, cur: Option<(LedgerSortKey, bool)>) -> HeadView {
    match kind {
        HeadKind::Sort(k) => match cur {
            Some((ck, desc)) if ck == k => HeadView {
                label: if desc {
                    format!("{title} ↓")
                } else {
                    format!("{title} ↑")
                },
                active: true,
                tip: if desc {
                    format!("{title}：当前倒序（↓），点击切换为正序")
                } else {
                    format!("{title}：当前正序（↑），点击切换为倒序")
                },
            },
            _ => HeadView {
                label: title.to_string(),
                active: false,
                tip: format!("点击按「{title}」排序（首次为倒序）"),
            },
        },
        HeadKind::ResetSort => HeadView {
            label: if cur.is_some() {
                format!("{title} ↺")
            } else {
                title.to_string()
            },
            active: cur.is_some(),
            tip: if cur.is_some() {
                "恢复初始排序（库返回的原始行序）".to_string()
            } else {
                "当前已是初始排序".to_string()
            },
        },
        HeadKind::Plain => HeadView {
            label: title.to_string(),
            active: false,
            tip: String::new(),
        },
    }
}

/// R-64：表头点击后的新排序状态。同列翻转方向；换列则从**倒序**重新开始（金额大的先看）。
pub fn next_sort_state(
    cur: Option<(LedgerSortKey, bool)>,
    key: LedgerSortKey,
) -> (LedgerSortKey, bool) {
    match cur {
        Some((k, desc)) if k == key => (k, !desc),
        _ => (key, true),
    }
}

/// 排序取值：统一折算成整数（金额取分、支付率取 bp），避免按显示文本比较。
/// `None` 只出现在未审计的合同上 —— 排序时恒定沉底、不参与数值比较（R-64）。
fn sort_value(s: &ctp_core::ledger::ContractSummary, key: LedgerSortKey) -> Option<i64> {
    match key {
        LedgerSortKey::Amount => Some(s.total_amount.cent()),
        LedgerSortKey::Audit => s.audit_price.map(|m| m.cent()),
        LedgerSortKey::Paid => Some(s.paid_amount.cent()),
        LedgerSortKey::Rate => Some(s.pay_rate.bp() as i64),
    }
}

/// R-64：按某一字段排序。`sort_by` 是稳定排序，等值行保持筛选原序，
/// 同一份数据反复切换方向不会把等值行的相对位置打乱。
fn sort_summaries(rows: &mut [ctp_core::ledger::ContractSummary], key: LedgerSortKey, desc: bool) {
    rows.sort_by(|a, b| match (sort_value(a, key), sort_value(b, key)) {
        (None, None) => std::cmp::Ordering::Equal,
        // 空值（未审计）不论正序倒序都沉底：已审计的才值得横向比较
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => {
            if desc {
                y.cmp(&x)
            } else {
                x.cmp(&y)
            }
        }
    });
}

enum Act {
    /// 点击行内任意位置（字段以外）：只把右侧栏切到该行，不打开任何弹窗（R-66）
    SelectRow(String),
    /// 点击合同名称：选中该行并打开只读详情弹窗
    DetailModal(String),
    /// 点击累计已付金额：选中并展开 / 收起拨款明细
    Expand(String),
    /// 点击承包单位：在该行下方展开 / 收起该单位的其他合同（R-59，按合同 id 记状态）
    ExpandContractor(String),
    /// 展开区里点拨款单号：打开单据详情
    OpenPayment(String),
    New,
    Activate,
    Audit,
    /// 项目交工：登记交工日期
    Delivery,
    ToPayments(String),
    /// 打开附件管理弹窗
    Attachments,
    /// 合同信息修改（R-50）：只改主档字段
    EditContract,
}

const STATUSES: [ContractStatus; 2] = [ContractStatus::Performing, ContractStatus::Closed];

/// 落位容差：小于这个位移不值得补，免得为亚像素反复触发滚动（R-61 / R-62）
const ANCHOR_EPSILON: f32 = 0.5;

/// 右侧栏不再随「是否选中」出现 / 消失，只有窗口窄到连 5 列都放不下时才让位。
/// 这个宽度是反推出来的：侧栏 344 + 间距 12 + 卡片内边距 34 + 5 列最小需求 514 ≈ 904（R-66）
const DETAIL_MIN_W: f32 = 920.0;

/// 右侧栏是否显示（R-66）。单独成函数是为了让阈值可以被测试钉住 ——
/// 这里一旦写反，要么窄窗口把表格挤爆，要么侧栏又变成「点了才有」。
fn detail_visible(avail_w: f32) -> bool {
    avail_w >= DETAIL_MIN_W
}

/// 进台账时的默认焦点（R-66）：已选中就保持不变，没选中就落到当前列表首行。
/// 只读纯函数，避免在绘制里写「如果没有选中就 select」这种容易每帧重查数据库的分支。
fn initial_focus(
    rows: &[ctp_core::ledger::ContractSummary],
    selected: Option<&str>,
) -> Option<String> {
    match selected {
        Some(id) => Some(id.to_string()),
        None => rows.first().map(|s| s.contract_id.clone()),
    }
}

/// 行高记忆的 key：整行可点区域必须在行内容之前注册（否则会抢走字段的点击），
/// 所以只能用上一帧实测的行高（R-66）
fn row_h_id() -> egui::Id {
    egui::Id::new("contract_ledger_row_h")
}

/// 首帧还没有实测值时用的兜底行高：单行文字 + 上下各 3px 内边距（R-66）
const ROW_H_FALLBACK: f32 = 26.0;

/// 点开一行的落位补偿量：正数表示内容需要向下移动（滚动条向上）。
///
/// 一次算清两件事：① 被点击的行要停在点击时的屏幕位置（上方面板收起会把它带走）；
/// ② 新展开的面板必须落在视口里 —— 行的位置钉死时面板只能往下长，长到视口下边界之外
/// 就得把内容往上推，否则展开的东西全在可视区之外、要手动滚才能看到。
/// 所以先定行的位置（放得下就钉回原位，放不下就贴顶），再把整块夹进视口（R-62）。
///
/// 单独成函数是为了让符号与边界可以被测试钉住 —— 这里写反会把面板推得更远。
fn settle_delta(
    row_top: f32,
    block_bottom: f32,
    clip_top: f32,
    clip_bottom: f32,
    anchor_top: f32,
) -> f32 {
    let view_h = (clip_bottom - clip_top).max(0.0);
    let block_h = (block_bottom - row_top).max(0.0);

    // ① 能整块放进视口：行回到点击时的位置；放不下：行贴视口顶部，尽可能多露面板
    let mut top = if block_h <= view_h {
        anchor_top
    } else {
        clip_top
    };

    // ② 整块往上夹：面板越出下边界时刚好贴着底边，不再多滚
    if block_h <= view_h && top + block_h > clip_bottom {
        top = clip_bottom - block_h;
    }
    // ③ 不越过上边界：点完之后用户又滚过一段时，锚点可能已经跑到视口外面
    if top < clip_top {
        top = clip_top;
    }

    top - row_top
}

/// 帧末是否要收起面板：只在点了面板以外的区域时才收，且「点展开入口」优先。
/// 同一帧既有空白点击又有展开意图时，交给 toggle 一次完成互斥切换，
/// 不走「先全部收起、再展开」的中间态（R-61）
fn should_collapse_panels(clicked_outside: bool, expanding: bool) -> bool {
    clicked_outside && !expanding
}

/// 切换展开目标时的偏移预修正量：被收起的面板在目标行**之上**才需要把内容补回来，
/// 在下方时目标行本来就不动。同一行（切换面板类型）和没有展开行时都是零（R-61）
fn offset_fix_for(open_idx: Option<usize>, clicked_idx: Option<usize>, panel_h: f32) -> f32 {
    match (open_idx, clicked_idx) {
        (Some(from), Some(to)) if from < to => panel_h,
        _ => 0.0,
    }
}

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;

    // 展开的拨款明细 / 单位合同面板：点击面板以外区域自动收起（用上一帧的矩形判断）。
    // R-61：这里**只判定、不改状态**，而且判定放在「释放」而不是「按下」。
    // 两个理由：① 收起会改变表格布局（少掉一个面板的高度，其下的行整体上移，靠近底部时滚动偏移
    // 还会被重新钳制），若在绘制与命中测试之前生效，按下那一帧的表格就已经不是用户看到的样子，
    // 命中落到错误的行上、指针微调还会把点击判成拖拽；② egui 的 clicked() 要到释放帧才成立，
    // 用按下帧判定只能拿到「点了面板外」、拿不到「点的是哪个展开入口」，于是仍会在按下与释放之间
    // 先塌一次。改到释放帧后，同一帧就能同时知道这两件事，展开与收起一次算清，中间不留空档。
    // 真正的状态转移统一放在帧末，见 `should_collapse_panels`。
    let keep_id = egui::Id::new("contract_expand_keep");
    let keep: Option<egui::Rect> = ui
        .memory(|m| m.data.get_temp::<Option<egui::Rect>>(keep_id))
        .flatten();
    let any_panel = app.expanded_contract.is_some() || app.expanded_contractor.is_some();
    let click_outside = any_panel
        && ui.input(|i| i.pointer.any_released())
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| keep.is_none_or(|r| !r.contains(pos)));
    page_header(
        ui,
        &p,
        "合同台账",
        "点击合同名称看详情；点击累计已付金额展开拨款明细；点击承包单位看该单位的其他合同；点击行内任意位置切换右侧栏",
    );

    let mut action: Option<Act> = None;
    // 本帧被点击的展开入口所在行的顶部纵坐标（屏幕坐标），用于下一帧的滚动锚定（R-61）
    let mut clicked_top: Option<f32> = None;
    // 同上，行序号：用来判断被收起的面板是否在这一行之上（在下方就不用补偿）
    let mut clicked_idx: Option<usize> = None;
    // R-33：合同台账导出（与屏幕表格同源的 ReportResult）
    let mut export_ledger = false;
    // 批量合同：导出空白模板 / 导入回传（与「拨款管理」的批量功能同构）
    let mut export_contract_tpl = false;
    let mut import_contracts = false;
    // 展开面板里的打印入口：与拨款管理页「拨款明细」操作列一致
    let mut print_pay: Option<ctp_core::payment::Payment> = None;
    let mut print_material: Option<String> = None;

    // ---------------- 工具栏
    ui.horizontal(|ui| {
        ui.add_sized(
            [250.0, 30.0],
            egui::TextEdit::singleline(&mut app.search)
                .hint_text("搜索编号 / 名称 / 承包单位")
                .margin(egui::Margin::symmetric(8.0, 4.0)),
        );

        let cur = app
            .status_filter
            .map(|s| s.label().to_string())
            .unwrap_or_else(|| "全部状态".into());
        egui::ComboBox::from_id_salt("status_filter")
            .width(120.0)
            .selected_text(cur)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(app.status_filter.is_none(), "全部状态")
                    .clicked()
                {
                    app.status_filter = None;
                }
                for s in STATUSES {
                    let sel = app.status_filter == Some(s);
                    if ui.selectable_label(sel, s.label()).clicked() {
                        app.status_filter = Some(s);
                    }
                }
            });

        ui.label(
            RichText::new(format!(
                "共 {} 份 · 已付 {} 元",
                app.summaries.len(),
                ctp_core::money::Money::from_cent(Money::sum_saturating(
                    app.summaries.iter().map(|s| s.paid_amount.cent())
                ))
                .format_yuan()
            ))
            .size(11.5)
            .color(p.text_muted),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, &p, "新建合同", true).clicked() {
                action = Some(Act::New);
            }
            if widgets::ghost_button(ui, &p, "批量导入合同")
                .on_hover_text("按模板填好后一次导入多份合同，逐行校验、失败行单独提示")
                .clicked()
            {
                import_contracts = true;
            }
            if widgets::ghost_button(ui, &p, "导出批量合同模板")
                .on_hover_text(
                    "导出含 21 列（= 新建合同全部字段）的空白 xlsx，带必填标记与枚举下拉",
                )
                .clicked()
            {
                export_contract_tpl = true;
            }
            if widgets::ghost_button(ui, &p, "导出 Excel").clicked() {
                export_ledger = true;
            }
        });
    });

    ui.add_space(8.0);

    // ---------------- 列表 + 详情
    let avail_w = ui.available_width();
    // R-50：详情卡放了 5 个操作按钮（附件 / 审计价 / 交工 / 信息修改 / 拨款），
    // 300 放不下，加宽到 344；列表最小宽按 748 反推阈值。
    let detail_w = 344.0_f32;
    // R-66：右侧栏始终可见（不再由「有没有选中」决定），窗口窄到装不下 5 列才让位。
    // 因为侧栏宽度恒定，选中 / 切换焦点不会再改变表格宽度 → 不会触发降列、也不会让面板消失。
    let show_detail = detail_visible(avail_w);
    let table_w = if show_detail {
        avail_w - detail_w - 12.0
    } else {
        avail_w
    };
    let page_h = ui.available_height();

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(table_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(table_w - 34.0);
                    let card_w = ui.available_width();
                    let rows = filtered_summaries(app);

                    if rows.is_empty() {
                        // R-61：空列表不会再有落位 / 预修正对象，清掉残留，避免筛选变化后误用旧坐标
                        app.pending_expand_settle = None;
                        app.pending_offset_fix = None;
                        widgets::empty_state(ui, &p, "没有符合条件的合同");
                        return;
                    }

                    // R-66：右侧栏始终可见 —— 没有选中任何合同时，焦点默认落在当前列表首行，
                    // 保证进页面就能看到一份合同。已选中（哪怕被搜索 / 状态筛选隐藏）时不动，
                    // 避免用户正在看的那份合同被悄悄换掉。
                    if let Some(id) = initial_focus(&rows, app.selected.as_deref()) {
                        if app.selected.as_deref() != Some(id.as_str()) {
                            app.select(&id);
                        }
                    }

                    // 右侧详情卡会把表格压窄：按「列宽需求」逐级降列，
                    // 避免固定列宽总和超出卡片，把详情卡挤出窗口右边界。
                    let c_no = 90.0;
                    let c_paid = 104.0;
                    let c_rate = 128.0;
                    let c_status = 72.0;
                    // 8 列（含承包单位/金额）891 / 7 列 714
                    // R-56 承包单位加宽到 1.5 倍（110 → 165），R-58 再 ×1.2 → 198：
                    // 两次加宽的差值一并计入阈值，8 列阈值同步上调 33
                    const NEED_8: f32 = 739.0 + 120.0 + 8.0 * 4.0 + 33.0;
                    const NEED_7: f32 = 574.0 + 112.0 + 7.0 * 4.0;
                    let (compact, tight_cols) = if card_w >= NEED_8 {
                        (false, false)
                    } else if card_w >= NEED_7 {
                        (true, false)
                    } else {
                        (true, true)
                    };
                    // 承包单位 R-58 再 ×1.2：165 → 198，多出的 33 由余量列「合同名称」让位
                    let c_contractor = if compact { 0.0 } else { 198.0 };
                    let c_amount = if compact { 0.0 } else { 92.0 };
                    let c_audit = if tight_cols { 0.0 } else { 92.0 };
                    let c_delivery = if tight_cols { 0.0 } else { 88.0 };
                    let cell_count = if tight_cols {
                        5.0
                    } else if compact {
                        7.0
                    } else {
                        8.0
                    };
                    let name_min = if tight_cols {
                        100.0
                    } else if compact {
                        112.0
                    } else {
                        120.0
                    };
                    let fixed = c_no
                        + c_contractor
                        + c_amount
                        + c_audit
                        + c_delivery
                        + c_paid
                        + c_rate
                        + c_status;
                    let name_w = (card_w - fixed - cell_count * 4.0).max(name_min);

                    // R-33：表头、单元格取值、合计行与 Excel 导出共用同一份 ReportResult。
                    // 窄屏降列只改变结果里的可见列，页面交互一律不动。
                    let visible = if tight_cols {
                        LedgerColumns::TIGHT
                    } else if compact {
                        LedgerColumns::COMPACT
                    } else {
                        LedgerColumns::FULL
                    };
                    let result = ReportResult::contract_ledger(&rows, visible);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for c in &result.columns {
                            let w = if c.key == "contract_name" {
                                name_w
                            } else {
                                c.screen_width
                            };
                            // R-64 / R-65：四个可排序列 +「合同编号」可点，其余列纯文本
                            let kind = head_kind(c.key);
                            let view = head_view(c.title, kind, app.ledger_sort);
                            if kind == HeadKind::Plain {
                                table_head(ui, &p, w, &view.label, c.right);
                            } else {
                                let color = if view.active { p.text } else { p.text_muted };
                                let clicked = table_head_clickable(
                                    ui, w, &view.label, color, c.right, c.key, view.tip,
                                )
                                .clicked();
                                if clicked {
                                    match kind {
                                        HeadKind::Sort(key) => app.toggle_ledger_sort(key),
                                        _ => app.reset_ledger_sort(),
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(2.0);

                    let mut detail: Option<String> = None;
                    let mut expand: Option<String> = None;
                    let mut expand_contractor: Option<String> = None;
                    // R-66：点中了行的空白处（没落到任何字段上）→ 只换右侧栏焦点
                    let mut clicked_row: Option<String> = None;
                    let mut open_pay: Option<String> = None;
                    let mut keep_rect: Option<egui::Rect> = None;
                    // 本帧处于展开态的行序号与它面板的高度（互斥，最多一个）
                    let mut open_idx: Option<usize> = None;
                    let mut open_panel_h: f32 = 0.0;

                    // R-61：切换展开目标时，被收起的面板会让它下方的行整体上移一个面板高度。
                    // 这里在构造列表之前就把偏移补掉，切换那一帧就不跳；
                    // 残余的落位（含把新面板推进视口）由面板画完后的 settle 一次算清（R-62）。
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("contract_list_scroll")
                        .auto_shrink([false, false])
                        .max_height((page_h - 64.0).max(220.0))
                        .min_scrolled_height((page_h - 110.0).max(200.0));
                    if std::mem::take(&mut app.ledger_scroll_top) {
                        // R-64：排序变了行序整体重排，停在原偏移只会落在无意义的位置，回顶部
                        scroll = scroll.vertical_scroll_offset(0.0);
                        app.ledger_offset = 0.0;
                    } else if let Some(fix) = app.pending_offset_fix.take() {
                        scroll = scroll.vertical_scroll_offset((app.ledger_offset - fix).max(0.0));
                    }
                    let scroll_out = scroll
                        .show(ui, |ui| {
                            for (idx, s) in rows.iter().enumerate() {
                                // 本行顶部（与上一帧同一套坐标），落位按它对齐（R-61 / R-62）
                                let row_top = ui.cursor().top();
                                // 本行面板的底边：没有展开面板时为 None（R-62）
                                let mut panel_bottom: Option<f32> = None;
                                // 本行所有显示文本来自 ReportResult；交互仍按列语义绘制
                                let cells = &result.rows[idx];
                                let text_of = |key: &str| -> String {
                                    result
                                        .columns
                                        .iter()
                                        .position(|c| c.key == key)
                                        .and_then(|i| cells.get(i))
                                        .map(|c| c.display())
                                        .unwrap_or_default()
                                };
                                let selected = app.selected.as_deref() == Some(&s.contract_id);
                                let expanded =
                                    app.expanded_contract.as_deref() == Some(&s.contract_id);
                                // R-59：该行承包单位是否处于展开态（只认当前行，单位同名行不一起展开）
                                let contractor_open =
                                    app.expanded_contractor.as_deref() == Some(&s.contract_id);
                                let mut paid_rect: Option<egui::Rect> = None;
                                let mut contractor_rect: Option<egui::Rect> = None;
                                let fill = if idx % 2 == 1 {
                                    p.surface_alt
                                } else {
                                    egui::Color32::TRANSPARENT
                                };
                                // R-66：整行可点 —— 点行内任意位置把右侧栏切到该行。
                                // 注册顺序必须在这里（行内容之前）：egui 的命中判定取「最后注册」的那个
                                // widget（hit_test.rs: hit_click = ...last()），先注册整行、再注册行内
                                // 字段，字段自然优先，落在空白 / 纯展示文字上时才由整行响应接住。
                                // 行高取上一帧实测值（单行高度固定），首帧用兜底值 —— 若按行间距一起算，
                                // 相邻两行的可点区会叠在一起，点边界会落到下一行。
                                let row_h = ui
                                    .memory(|m| m.data.get_temp::<f32>(row_h_id()))
                                    .unwrap_or(ROW_H_FALLBACK);
                                let row_rect = egui::Rect::from_min_size(
                                    egui::pos2(ui.cursor().left(), row_top),
                                    egui::vec2(card_w.max(1.0), row_h),
                                );
                                let row_resp = ui
                                    .interact(
                                        row_rect,
                                        ui.id().with(("ledger_row", &s.contract_id)),
                                        egui::Sense::click(),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                let row_frame = egui::Frame::none()
                                    .fill(fill)
                                    .inner_margin(egui::Margin::symmetric(0.0, 3.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            table_cell(ui, c_no, false, |ui| {
                                                // R-60：合同编号不再承担任何点击交互，
                                                // 也不再使用主色，避免看起来仍像链接
                                                let mut txt = RichText::new(text_of("contract_no"))
                                                    .size(12.5)
                                                    .color(p.text);
                                                if selected {
                                                    txt = txt.strong().underline();
                                                }
                                                ui.label(txt);
                                            });
                                            table_cell(ui, name_w, false, |ui| {
                                                let cname = text_of("contract_name");
                                                // R-56：草稿视为「未生效」，名称用警示色；
                                                // 选中态仍用主色（交互反馈优先），其余状态不变。
                                                let name_color = if selected {
                                                    p.primary
                                                } else if s.status == ContractStatus::Draft.code() {
                                                    p.danger
                                                } else {
                                                    p.text
                                                };
                                                // R-60：名称改为「查看详情」入口，展开标记已移到累计已付
                                                let r = widgets::marquee_sense(
                                                    ui,
                                                    &cname,
                                                    (name_w - 8.0).max(20.0),
                                                    12.5,
                                                    name_color,
                                                    egui::Sense::click(),
                                                )
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .on_hover_text(
                                                    "点击查看合同详情；名称过长时悬停自动滚动",
                                                );
                                                if r.clicked() {
                                                    detail = Some(s.contract_id.clone());
                                                }
                                            });
                                            if !compact {
                                                table_cell(ui, c_contractor, false, |ui| {
                                                    // R-59：承包单位是「该单位其他合同」的入口；
                                                    // 展开时用主色，便于一眼看出当前展开的是哪一行
                                                    let contractor = s.contractor_name.clone();
                                                    let is_open = matches!(
                                                        app.expanded_contractor.as_deref(),
                                                        Some(id) if id == s.contract_id
                                                    );
                                                    let color = if is_open {
                                                        p.primary
                                                    } else {
                                                        p.text_muted
                                                    };
                                                    // R-60：与「展开拨款明细」同款标记，手感统一；
                                                    // 行内 item_spacing 被清零过，这里留 4px 免得「＋」贴住单位名
                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                    let mark = ui
                                                        .add(
                                                            egui::Label::new(
                                                                RichText::new(if is_open {
                                                                    "－"
                                                                } else {
                                                                    "＋"
                                                                })
                                                                .size(11.0)
                                                                .color(p.text_muted),
                                                            )
                                                            .sense(egui::Sense::click()),
                                                        )
                                                        .on_hover_cursor(
                                                            egui::CursorIcon::PointingHand,
                                                        );
                                                    let r = widgets::marquee_sense(
                                                        ui,
                                                        &contractor,
                                                        (c_contractor - 23.0).max(20.0),
                                                        12.0,
                                                        color,
                                                        egui::Sense::click(),
                                                    )
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text(
                                                        "点击展开 / 收起该单位的其他合同；名称过长时悬停自动滚动",
                                                    );
                                                    if r.clicked() || mark.clicked() {
                                                        expand_contractor =
                                                            Some(s.contract_id.clone());
                                                        clicked_top = Some(row_top);
                                                        clicked_idx = Some(idx);
                                                    }
                                                    if is_open {
                                                        contractor_rect =
                                                            Some(r.rect.union(mark.rect));
                                                    }
                                                });
                                                table_cell(ui, c_amount, true, |ui| {
                                                    ui.label(
                                                        RichText::new(text_of("total_amount"))
                                                            .size(12.5)
                                                            .color(p.text),
                                                    );
                                                });
                                            }
                                            if !tight_cols {
                                                table_cell(ui, c_audit, true, |ui| {
                                                    ui.label(
                                                        RichText::new(text_of("audit_price"))
                                                            .size(12.5)
                                                            .color(if s.is_audited {
                                                                p.text
                                                            } else {
                                                                p.text_muted
                                                            }),
                                                    );
                                                });
                                                table_cell(ui, c_delivery, false, |ui| {
                                                    ui.label(
                                                        RichText::new(text_of("delivery_date"))
                                                            .size(12.0)
                                                            .color(if s.delivery_date.is_some() {
                                                                p.text
                                                            } else {
                                                                p.text_muted
                                                            }),
                                                    );
                                                });
                                            }
                                            table_cell(ui, c_paid, true, |ui| {
                                                // R-60：累计已付成为「展开拨款明细」的入口。
                                                // 右对齐布局里先放金额（最右），后放标记（落在金额左侧）。
                                                // 行内 item_spacing 被清零过，这里单独留 4px，免得「－」贴住数字。
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                let amount = ui
                                                    .add(
                                                        egui::Label::new(
                                                            RichText::new(text_of("paid_amount"))
                                                                .size(12.5)
                                                                .color(p.text),
                                                        )
                                                        .sense(egui::Sense::click()),
                                                    )
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text(
                                                        "点击展开 / 收起该合同的拨款明细",
                                                    );
                                                let mark = ui
                                                    .add(
                                                        egui::Label::new(
                                                            RichText::new(if expanded {
                                                                "－"
                                                            } else {
                                                                "＋"
                                                            })
                                                            .size(11.0)
                                                            .color(p.text_muted),
                                                        )
                                                        .sense(egui::Sense::click()),
                                                    )
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text(
                                                        "点击展开 / 收起该合同的拨款明细",
                                                    );
                                                    if amount.clicked() || mark.clicked() {
                                                        expand = Some(s.contract_id.clone());
                                                        clicked_top = Some(row_top);
                                                        clicked_idx = Some(idx);
                                                    }
                                                if expanded {
                                                    paid_rect = Some(amount.rect.union(mark.rect));
                                                }
                                            });
                                            table_cell(ui, c_rate, false, |ui| {
                                                let frac = s.pay_rate.bp() as f32 / 10_000.0;
                                                let color = if s.is_inverted {
                                                    p.danger
                                                } else if frac > 0.99 {
                                                    p.warn
                                                } else {
                                                    p.primary
                                                };
                                                widgets::bar(ui, &p, frac, color, 82.0, 7.0);
                                                ui.label(
                                                    RichText::new(text_of("pay_rate"))
                                                        .size(11.5)
                                                        .color(p.text_muted),
                                                );
                                            });
                                            table_cell(ui, c_status, false, |ui| {
                                                let (_, tone) = project_status(s);
                                                let st = text_of("project_status");
                                                widgets::badge(ui, &p, &st, tone);
                                            });
                                        });
                                    });
                                // 本行实测高度留给下一帧的整行可点区域（R-66）
                                ui.memory_mut(|m| {
                                    m.data
                                        .insert_temp(row_h_id(), row_frame.response.rect.height())
                                });
                                if row_resp.clicked() {
                                    clicked_row = Some(s.contract_id.clone());
                                }
                                if expanded {
                                    let panel = expand_panel(
                                        ui,
                                        &p,
                                        s,
                                        &app.payments,
                                        &mut open_pay,
                                        &mut print_pay,
                                        &mut print_material,
                                    );
                                    keep_rect = Some(paid_rect.map_or(panel, |n| n.union(panel)));
                                    open_idx = Some(idx);
                                    open_panel_h = panel.height();
                                    panel_bottom = Some(panel.bottom());
                                }
                                if contractor_open {
                                    // R-59：取全量台账（不受搜索 / 状态筛选影响），排除当前行
                                    let others = other_contracts_of(
                                        &app.summaries,
                                        &s.contractor_name,
                                        &s.contract_id,
                                    );
                                    let panel = contractor_panel(
                                        ui,
                                        &p,
                                        s,
                                        &others,
                                        &mut detail,
                                    );
                                    let union = contractor_rect.map_or(panel, |c| c.union(panel));
                                    keep_rect = Some(keep_rect.map_or(union, |k| k.union(union)));
                                    open_idx = Some(idx);
                                    open_panel_h = panel.height();
                                    panel_bottom = Some(panel.bottom());
                                }
                                // R-62：落位。必须放在面板绘制之后 —— 面板里的 TableBuilder
                                // 自带一层 ScrollArea，它在自己的 end() 里会无条件 take 掉本帧的
                                // 滚动指令（egui 0.29 scroll_area.rs:820），在行顶部发出的补偿
                                // 会被这层内嵌滚动区吞掉，只有这里才真正落到台账列表自己身上。
                                if let (Some(panel_bottom), Some((id, anchor_top))) =
                                    (panel_bottom, app.pending_expand_settle.as_ref())
                                {
                                    if id == &s.contract_id {
                                        let clip = ui.clip_rect();
                                        let delta = settle_delta(
                                            row_top,
                                            panel_bottom,
                                            clip.top(),
                                            clip.bottom(),
                                            *anchor_top,
                                        );
                                        if delta.abs() > ANCHOR_EPSILON {
                                            ui.scroll_with_delta_animation(
                                                egui::vec2(0.0, delta),
                                                egui::style::ScrollAnimation::none(),
                                            );
                                        }
                                    }
                                }
                            }
                        });
                    app.ledger_offset = scroll_out.state.offset.y;

                    // R-33：合计行取自 result.totals，列宽与对齐跟表头同一套列配置
                    widgets::divider(ui, &p);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (ci, cell) in result.totals.iter().enumerate() {
                            let c = &result.columns[ci];
                            let w = if c.key == "contract_name" {
                                name_w
                            } else {
                                c.screen_width
                            };
                            table_cell(ui, w, c.right, |ui| {
                                ui.label(
                                    RichText::new(cell.display())
                                        .size(12.5)
                                        .strong()
                                        .color(p.text),
                                );
                            });
                        }
                    });

                    ui.memory_mut(|m| m.data.insert_temp(keep_id, keep_rect));
                    // R-62：落位目标只服务一帧 —— 绘制时已按它补偿过（或该行不在结果里），到此清掉
                    app.pending_expand_settle = None;
                    // R-61：被收起的面板在这一行之上，下一帧把这一个面板的高度补回去
                    let fix = offset_fix_for(open_idx, clicked_idx, open_panel_h);
                    if fix > 0.0 {
                        app.pending_offset_fix = Some(fix);
                    }

                    // R-66：行内点击的优先级低于字段点击 —— 字段已经接管了这一帧的点击
                    // （egui 一次按键只判给一个 widget），这里只兜住落在空白处的那些。
                    if let Some(id) = expand {
                        action = Some(Act::Expand(id));
                    } else if let Some(id) = expand_contractor {
                        action = Some(Act::ExpandContractor(id));
                    } else if let Some(id) = detail {
                        action = Some(Act::DetailModal(id));
                    } else if let Some(id) = clicked_row {
                        action = Some(Act::SelectRow(id));
                    }
                    if let Some(id) = open_pay {
                        action = Some(Act::OpenPayment(id));
                    }
                });
            },
        );

        if show_detail {
            ui.add_space(12.0);
            let max_h = (page_h - 8.0).max(260.0);
            ui.allocate_ui_with_layout(
                egui::vec2(detail_w, max_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    // 窗口高度不足时整卡可滚动，避免底部操作按钮被状态栏压住
                    egui::ScrollArea::vertical()
                        .id_salt("contract_detail_scroll")
                        .max_height(max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(detail_w - 2.0);
                            detail_card(app, ui, &mut action, max_h);
                        });
                },
            );
        }
    });

    // R-46：导出前先选列。这里按当前搜索词 / 状态筛选后的同一份行序构造完整 9 列结果，
    // 快照进弹窗；用户在弹窗里勾选与调序后，Excel 只写所选列、顺序与选择一致。
    // 数据范围、入口按钮、权限与「导出失败」提示链路都沿用原逻辑。
    if export_ledger {
        let rows = filtered_summaries(app);
        let result = ReportResult::contract_ledger(&rows, LedgerColumns::FULL);
        app.open_export_columns(result);
    }

    // 批量合同：模板 21 列 = 新建合同表单的全部字段；导入每行独立事务、失败行单独提示。
    // 与「拨款管理」的批量入口共用体检（校验表 / 上限 / 结果弹窗），只是落库模块不同。
    if export_contract_tpl {
        let name = format!("批量合同模板_{}.xlsx", ctp_core::date::today_local_iso());
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&name)
            .add_filter("Excel 工作簿", &["xlsx"])
            .save_file()
        {
            match ctp_app::contract_batch::write_template(&path, &ctp_core::date::now_iso()) {
                Ok(n) => app.success(format!(
                    "模板已导出（{n} 列）：带 * 为必填，枚举列可下拉选择；填好后用「批量导入合同」导入"
                )),
                Err(e) => app.error(format!("导出模板失败：{e}")),
            }
        }
    }
    if import_contracts && app.require_write() {
        if app.busy.is_some() {
            app.toast(Tone::Neutral, "正在处理上一次导入，请稍候");
        } else if let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel 工作簿", &["xlsx"])
            .pick_file()
        {
            app.pending_import = Some((crate::BatchImport::Contracts, path));
            app.busy = Some(crate::BatchImport::Contracts.busy_text().to_string());
        }
    }

    // 浏览用户不许触发写类动作：不置灰，点了直接提示「无权限」
    // ---------------- 帧末面板状态转移（R-61）
    // 绘制与命中测试全部结束之后再改状态：本帧的布局、命中与点击判定都用用户实际看到的那一版。
    // 若在帧首收起，按下那一帧的表格就少了一个面板的高度，其下的行整体移位、靠近底部时滚动偏移
    // 还会被重新钳制，指针落点与新的行错位，点击（甚至被追着微调成拖拽）便落不到目标上。
    // 收起与展开在同一帧末完成，下一帧一次性呈现「旧面板消失 + 新面板出现」，没有中间空档。
    let expanding = matches!(
        action,
        Some(Act::Expand(_)) | Some(Act::ExpandContractor(_))
    );
    if should_collapse_panels(click_outside, expanding) {
        app.expanded_contract = None;
        app.expanded_contractor = None;
    }

    let writes = matches!(
        action,
        Some(Act::New)
            | Some(Act::Activate)
            | Some(Act::Audit)
            | Some(Act::Delivery)
            | Some(Act::EditContract)
    );
    if writes && !app.require_write() {
        return;
    }

    match action {
        Some(Act::SelectRow(id)) => {
            // R-66：点中行的空白处只换右侧栏焦点 —— 不展开、不弹窗、不动滚动位置
            app.select(&id);
        }
        Some(Act::DetailModal(id)) => {
            // R-60：点合同名称看详情时同时选中该行，右侧详情卡与弹窗保持同一份合同
            app.select(&id);
            app.open_contract_detail(&id);
        }
        Some(Act::Expand(id)) => {
            app.select(&id);
            // R-62：每一次「点开」都记落位目标。首次展开虽然没有旧面板可参照，
            // 面板同样可能长在视口之外；只有再点一次收起（本行原本已展开）才不需要定位。
            if app.expanded_contract.as_deref() != Some(id.as_str()) {
                if let Some(top) = clicked_top {
                    app.pending_expand_settle = Some((id.clone(), top));
                }
            }
            app.toggle_expand(&id);
        }
        Some(Act::ExpandContractor(id)) => {
            // R-66：字段点击也同步切换焦点。侧栏宽度恒定后，选中不再引发降列，
            // R-59 当初「不 select 以免面板消失」的顾虑已经不成立。
            app.select(&id);
            if app.expanded_contractor.as_deref() != Some(id.as_str()) {
                if let Some(top) = clicked_top {
                    app.pending_expand_settle = Some((id.clone(), top));
                }
            }
            app.toggle_contractor(&id);
        }
        Some(Act::OpenPayment(id)) => app.open_payment(&id),
        Some(Act::New) => {
            app.new_contract = crate::dialogs::ContractForm::default();
            app.dialog = crate::dialogs::Dialog::NewContract;
        }
        Some(Act::Activate) => {
            if let Some(id) = app.selected.clone() {
                let op = app.operator();
                match app.exec(|conn| service::activate_contract(conn, &id, op)) {
                    Ok(c) => {
                        app.refresh();
                        app.select(&c.id);
                        app.success(format!("合同 {} 已生效，可以登记拨款", c.contract_no));
                    }
                    Err(e) => app.error(format!("生效失败：{e}")),
                }
            }
        }
        Some(Act::Attachments) => {
            if let Some(id) = app.selected.clone() {
                app.open_attachments(&id);
            }
        }
        Some(Act::EditContract) => {
            if let Some(id) = app.selected.clone() {
                app.open_edit_contract(&id);
            }
        }
        Some(Act::Audit) => {
            // R-67：整表回填交给 App 统一读库（摘要没有审定日期 / 单位 / 报告号，不能用来预填）
            if let Some(id) = app.selected.clone() {
                app.open_audit_price(&id);
            }
        }
        Some(Act::Delivery) => {
            app.delivery_form = app
                .detail
                .as_ref()
                .and_then(|d| d.delivery_date.clone())
                .unwrap_or_else(ctp_core::date::today_iso);
            app.dialog = crate::dialogs::Dialog::DeliveryDate;
        }
        Some(Act::ToPayments(id)) => {
            app.pay_contract = Some(id);
            app.screen = crate::Screen::Payments;
        }
        None => {}
    }

    if let Some(pm) = print_pay {
        app.open_print_preview(&pm);
    }
    if let Some(cid) = print_material {
        app.open_material_print(&cid);
    }
}

/// 当前搜索词 + 项目状态筛选后的合同行（屏幕表格与导出共用同一份筛选结果）。
fn filtered_summaries(app: &App) -> Vec<ctp_core::ledger::ContractSummary> {
    let kw = app.search.trim().to_lowercase();
    let mut out: Vec<ctp_core::ledger::ContractSummary> = app
        .summaries
        .iter()
        .filter(|s| {
            let hit = kw.is_empty()
                || s.contract_no.to_lowercase().contains(&kw)
                || s.contract_name.to_lowercase().contains(&kw)
                || s.contractor_name.to_lowercase().contains(&kw);
            let st = match app.status_filter {
                Some(ContractStatus::Performing) => s.pay_rate.bp() < 10_000,
                Some(ContractStatus::Closed) => s.pay_rate.bp() >= 10_000,
                _ => true,
            };
            hit && st
        })
        .cloned()
        .collect();
    // R-64：屏幕表格、合计行与 Excel 导出都从这一份行序出发，排序只能在这一层做，
    // 三处才不会出现「屏幕变了、导出没变」。展开面板按行 id 记状态，跟着它那一行走。
    if let Some((key, desc)) = app.ledger_sort {
        sort_summaries(&mut out, key, desc);
    }
    out
}

/// 项目状态：支付率达 100% 自动「归档关闭」，否则「履行中」。
fn project_status(s: &ctp_core::ledger::ContractSummary) -> (&'static str, Tone) {
    if s.is_archived() {
        ("归档关闭", Tone::Neutral)
    } else {
        ("履行中", Tone::Success)
    }
}

/// 固定宽度的表格单元（表头与数据行共用，保证列对齐）。
fn table_cell(ui: &mut egui::Ui, width: f32, right: bool, add: impl FnOnce(&mut egui::Ui)) {
    let layout = if right {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(egui::vec2(width, 26.0), layout, |ui| {
        ui.set_width(width);
        // 每列留 4px 内侧留白，避免相邻列文字贴在一起
        ui.add_space(4.0);
        add(ui);
    });
}

fn table_head(ui: &mut egui::Ui, p: &crate::theme::Palette, width: f32, text: &str, right: bool) {
    table_cell(ui, width, right, |ui| {
        ui.label(RichText::new(text).size(12.0).color(p.text_muted));
    });
}

/// R-64 / R-65：可点表头。点按**只落在文字上**（用户确认的口径）——
/// 表格本身不拦点击，文字左边的留白与上下边距都不响应。
///
/// 这里刻意不做「整格一个 response」：`Label` 默认是可选中的文本，可选中的 label 点按语义是
/// `click_and_drag`，会与外层格子的点击互相争抢，文字上按下去反而拿不到 `clicked()`
/// （表现为「只有文字旁边的空白能排序」，且右对齐列在左、左对齐列在右）。
/// 所以 label 一律 `selectable(false)` 退回纯展示，再按它自己的矩形单独注册一次点击。
/// 箭头与文字拼在同一个字符串里：右对齐列是 `right_to_left` 布局，分开放会把箭头排到文字左边。
fn table_head_clickable(
    ui: &mut egui::Ui,
    width: f32,
    text: &str,
    color: egui::Color32,
    right: bool,
    key: &str,
    tip: String,
) -> egui::Response {
    let layout = if right {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    let inner = ui.allocate_ui_with_layout(egui::vec2(width, 26.0), layout, |ui| {
        ui.set_width(width);
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).selectable(false))
    });
    // 只认文字矩形：留白不响应，鼠标移上去才变手型
    ui.interact(
        inner.inner.rect,
        egui::Id::new(("ledger_head", key)),
        egui::Sense::click(),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text(tip)
}

/// 合同行下方展开的拨款明细面板：表头与「拨款管理 · 拨款明细」一致，已作废条目不再列出。
fn expand_panel(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    s: &ctp_core::ledger::ContractSummary,
    payments: &[ctp_core::payment::Payment],
    open_pay: &mut Option<String>,
    print_pay: &mut Option<ctp_core::payment::Payment>,
    print_material: &mut Option<String>,
) -> egui::Rect {
    let mut mine: Vec<&ctp_core::payment::Payment> = payments
        .iter()
        .filter(|pm| {
            pm.contract_id == s.contract_id && pm.status != ctp_core::payment::PaymentStatus::Void
        })
        .collect();
    mine.sort_by(|a, b| {
        b.pay_date
            .cmp(&a.pay_date)
            .then(b.payment_no.cmp(&a.payment_no))
    });

    let start = ui.min_rect();
    egui::Frame::none()
        .fill(p.primary_soft)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("拨款明细 · {} 笔", mine.len()))
                        .size(12.0)
                        .strong()
                        .color(p.text),
                );
                if !mine.is_empty() {
                    let net: i64 = Money::sum_saturating(mine.iter().map(|x| x.amount_cent.cent()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "净额 {} 元",
                                Money::from_cent(net).format_yuan()
                            ))
                            .size(11.5)
                            .color(p.text_muted),
                        );
                    });
                }
            });
            ui.add_space(4.0);

            if mine.is_empty() {
                ui.label(
                    RichText::new("该合同暂无有效拨款记录")
                        .size(11.5)
                        .color(p.text_muted),
                );
                return;
            }

            // R-33：与「拨款管理 · 拨款明细」同源，列与取值都来自 ReportResult；
            // 操作列属于交互，仍留在页面侧绘制。
            let owned: Vec<ctp_core::payment::Payment> =
                mine.iter().take(8).map(|p| (*p).clone()).collect();
            let result = ReportResult::payment_ledger(&owned, &s.contract_name);
            ui.spacing_mut().item_spacing.x = 12.0;
            let mut tb = TableBuilder::new(ui)
                .striped(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for c in &result.columns {
                tb = tb.column(if c.screen_remainder {
                    Column::remainder().at_least(c.screen_width)
                } else {
                    Column::auto().at_least(c.screen_width)
                });
            }
            tb = tb.column(Column::auto().at_least(150.0));
            tb.header(22.0, |mut h| {
                for c in &result.columns {
                    h.col(|ui| {
                        ui.label(RichText::new(c.title).size(11.5).color(p.text_muted));
                    });
                }
                h.col(|ui| {
                    ui.label(RichText::new("操作").size(11.5).color(p.text_muted));
                });
            })
            .body(|mut body| {
                for (ri, pm) in mine.iter().take(8).enumerate() {
                    let cells = &result.rows[ri];
                    body.row(28.0, |mut row| {
                        for (ci, cell) in cells.iter().enumerate() {
                            let key = result.columns[ci].key;
                            let text = cell.display();
                            row.col(|ui| match key {
                                "payment_no" => {
                                    if widgets::link(ui, p, text.as_str(), 11.5)
                                        .on_hover_text("查看拨款单详情")
                                        .clicked()
                                    {
                                        *open_pay = Some(pm.id.clone());
                                    }
                                }
                                "amount" => {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(text.as_str())
                                                    .size(12.0)
                                                    .color(p.text),
                                            );
                                        },
                                    );
                                }
                                "reason" => {
                                    widgets::cell_marquee(
                                        ui,
                                        text.as_str(),
                                        11.5,
                                        p.text_muted,
                                        egui::Align::LEFT,
                                    );
                                }
                                "status" => {
                                    widgets::badge(ui, p, text.as_str(), Tone::Success);
                                }
                                _ => {
                                    ui.label(
                                        RichText::new(text.as_str()).size(11.5).color(p.text_muted),
                                    );
                                }
                            });
                        }
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if widgets::link(ui, p, "打印拨款单", 11.0)
                                .on_hover_text("预览付款打印模板的 9 个合并域取值")
                                .clicked()
                            {
                                *print_pay = Some((*pm).clone());
                            }
                            if widgets::link(ui, p, "打印附件", 11.0)
                                .on_hover_text("中标通知书、审计报告等材料打印")
                                .clicked()
                            {
                                *print_material = Some(pm.contract_id.clone());
                            }
                        });
                    });
                }
            });

            if mine.len() > 8 {
                ui.add_space(2.0);
                ui.label(
                    RichText::new("仅显示最近 8 笔，完整流水见「拨款管理」")
                        .size(10.5)
                        .color(p.text_muted),
                );
            }
        });
    ui.add_space(3.0);
    start.union(ui.min_rect())
}

/// R-59：同一承包单位的其他合同——按承包单位名称在全量台账里取，排除当前行。
/// 单位名称在字典里不是唯一键，同名单位会被并到一起（台账本就按名称展示，界面无法再区分）。
fn other_contracts_of<'a>(
    all: &'a [ctp_core::ledger::ContractSummary],
    contractor_name: &str,
    current_id: &str,
) -> Vec<&'a ctp_core::ledger::ContractSummary> {
    all.iter()
        .filter(|o| o.contractor_name == contractor_name && o.contract_id != current_id)
        .collect()
}

/// R-63：承包单位面板的「合计合同金额」口径 —— 本合同金额 + 该单位其他合同金额之和。
/// 台账合计行、总览卡片、报表都统计别的东西，只有这里代表「这家单位在库里的全部合同额」，
/// 所以本合同必须计入，否则同一块面板里「份数含本合同、金额不含」会自相矛盾。
/// 单独成函数是为了让这个加法只有一处实现，界面改动不会漏掉本合同。
fn contractor_total_cent(own_cent: i64, others: &[&ctp_core::ledger::ContractSummary]) -> i64 {
    Money::sum_saturating(
        std::iter::once(own_cent).chain(others.iter().map(|o| o.total_amount.cent())),
    )
}

/// R-59：点承包单位后在该行下方展开的面板，沿用「拨款明细」展开卡的同款样式。
/// `own` 是当前行合同：既是面板内表格 id 作用域的锚点（同一帧里若有多张表，自动生成的
/// id 会撞车，egui 会报 “First/Second use of Table ID”），也是 R-63 合计口径里的「本合同」。
/// 列：合同编号 / 合同名称 / 合同金额 / 审计价 / 支付率 / 项目状态；点名称打开合同详情
/// （R-60：编号与台账一致，不再可点）。
fn contractor_panel(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    own: &ctp_core::ledger::ContractSummary,
    others: &[&ctp_core::ledger::ContractSummary],
    open: &mut Option<String>,
) -> egui::Rect {
    let start = ui.min_rect();
    ui.push_id(("contractor_panel", &own.contract_id), |ui| {
        contractor_panel_body(ui, p, own, others, open);
    });
    ui.add_space(3.0);
    start.union(ui.min_rect())
}

fn contractor_panel_body(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    own: &ctp_core::ledger::ContractSummary,
    others: &[&ctp_core::ledger::ContractSummary],
    open: &mut Option<String>,
) -> egui::Rect {
    let start = ui.min_rect();
    egui::Frame::none()
        .fill(p.primary_soft)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // R-63：份数与金额同口径 —— 都含本合同，避免「N 份」却按 N+1 份算钱
                ui.label(
                    RichText::new(format!("承包单位合同 · {} 份", others.len() + 1))
                        .size(12.0)
                        .strong()
                        .color(p.text),
                );
                let total = contractor_total_cent(own.total_amount.cent(), others);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "合计合同金额 {} 元",
                            Money::from_cent(total).format_yuan()
                        ))
                        .size(11.5)
                        .color(p.text_muted),
                    );
                });
            });
            ui.add_space(4.0);

            if others.is_empty() {
                ui.label(
                    RichText::new(format!("{} 名下暂无其他合同", own.contractor_name))
                        .size(11.5)
                        .color(p.text_muted),
                );
                return;
            }

            ui.spacing_mut().item_spacing.x = 12.0;
            let tb = TableBuilder::new(ui)
                .striped(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::auto().at_least(90.0))
                .column(Column::remainder().at_least(120.0))
                .column(Column::auto().at_least(104.0))
                .column(Column::auto().at_least(96.0))
                .column(Column::auto().at_least(84.0))
                .column(Column::auto().at_least(76.0));
            tb.header(22.0, |mut h| {
                for (title, right) in [
                    ("合同编号", false),
                    ("合同名称", false),
                    ("合同金额", true),
                    ("审计价", true),
                    ("支付率", true),
                    ("项目状态", false),
                ] {
                    h.col(|ui| {
                        let text = RichText::new(title).size(11.5).color(p.text_muted);
                        if right {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(text);
                                },
                            );
                        } else {
                            ui.label(text);
                        }
                    });
                }
            })
            .body(|mut body| {
                for o in others {
                    body.row(24.0, |mut row| {
                        row.col(|ui| {
                            // R-60：与台账一致，编号不做任何点击交互，也不再用主色
                            ui.label(RichText::new(&o.contract_no).size(11.5).color(p.text_muted));
                        });
                        row.col(|ui| {
                            let w = ui.available_width().max(20.0);
                            let r = widgets::marquee_sense(
                                ui,
                                &o.contract_name,
                                w,
                                11.5,
                                p.text,
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("查看合同详情；名称过长时悬停自动滚动");
                            if r.clicked() {
                                *open = Some(o.contract_id.clone());
                            }
                        });
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(o.total_amount.format_yuan())
                                            .size(11.5)
                                            .color(p.text),
                                    );
                                },
                            );
                        });
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(if o.is_audited {
                                            o.audit_price
                                                .map(|v| v.format_yuan())
                                                .unwrap_or_else(|| "—".into())
                                        } else {
                                            "—".into()
                                        })
                                        .size(11.5)
                                        .color(if o.is_audited { p.text } else { p.text_muted }),
                                    );
                                },
                            );
                        });
                        row.col(|ui| {
                            let frac = o.pay_rate.bp() as f32 / 10_000.0;
                            let color = if o.is_inverted {
                                p.danger
                            } else if frac > 0.99 {
                                p.warn
                            } else {
                                p.primary
                            };
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(o.pay_rate.percent_string())
                                            .size(11.5)
                                            .color(color),
                                    );
                                },
                            );
                        });
                        row.col(|ui| {
                            let (st, tone) = project_status(o);
                            widgets::badge(ui, p, st, tone);
                        });
                    });
                }
            });
        });
    start.union(ui.min_rect())
}

fn detail_card(app: &App, ui: &mut egui::Ui, action: &mut Option<Act>, max_h: f32) {
    let p = app.pal;
    let Some(s) = app.selected_summary() else {
        // R-66：侧栏始终占位 —— 列表为空（筛选无结果 / 库为空）时给一句说明，
        // 不留一块没有边界的空白，也不让「侧栏时有时无」回归
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("未选择合同")
                    .size(13.0)
                    .strong()
                    .color(p.text_muted),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("点击左侧表格任意一行，这里显示该合同的金额、支付率与操作入口。")
                    .size(11.5)
                    .color(p.text_muted),
            );
        });
        return;
    };
    let d = app.detail.as_ref();
    let is_draft = s.status == ContractStatus::Draft.code();
    let can_pay = ContractStatus::from_code(&s.status)
        .map(|x| x.is_payable())
        .unwrap_or(false);
    // 窗口较矮时收起次要字段与最近拨款，保证底部操作按钮不会被窗口下沿裁掉
    let tight = max_h < 620.0;
    // 字段行较多，行距收紧到 7px，把内容压在卡片高度内
    ui.spacing_mut().item_spacing.y = 7.0;

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&s.contract_no)
                    .size(15.0)
                    .strong()
                    .color(p.text),
            );
            let (pstatus, ptone) = project_status(s);
            widgets::badge(ui, &p, pstatus, ptone);
            // 标题行右侧放「设为生效 / 作废」：前者是草稿态主行动，
            // 后者是低频危险操作，都不占底部操作区，避免按钮换行把卡片撑高
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_draft && widgets::primary_button_sm(ui, &p, "设为生效").clicked() {
                    *action = Some(Act::Activate);
                }
            });
        });
        ui.label(
            RichText::new(&s.contract_name)
                .size(12.5)
                .color(p.text_muted),
        );
        ui.add_space(8.0);
        widgets::divider(ui, &p);

        widgets::kv(ui, &p, "承包单位", &s.contractor_name);
        widgets::kv(
            ui,
            &p,
            "合同金额",
            &format!("{} 元", s.total_amount.format_yuan()),
        );
        widgets::kv(
            ui,
            &p,
            "审计价",
            &s.audit_price
                .map(|m| format!("{} 元", m.format_yuan()))
                .unwrap_or_else(|| "未审定".into()),
        );
        widgets::kv(
            ui,
            &p,
            "有效计价基准",
            &format!("{} 元", s.base_amount.format_yuan()),
        );
        widgets::kv(
            ui,
            &p,
            "累计已付",
            &format!("{} 元", s.paid_amount.format_yuan()),
        );
        // 可付额度按「付清条件」封顶后的口径，与拨款管理页一致
        let quota_view = s.to_quota(ctp_core::rate::Rate::ZERO);
        let payable_now = Money::from_cent(quota_view.payable_amount.cent().max(0));
        widgets::kv(
            ui,
            &p,
            "可付额度",
            &format!("{} 元", payable_now.format_yuan()),
        );
        // 支付率后带上封顶比例（97%）：说明付清条件尚未满足（不额外占一行）
        let rate_text = if quota_view.limit_note.is_some() {
            format!(
                "{}（上限 {}%）",
                s.pay_rate.percent_string(),
                quota_view.cap_bp / 100
            )
        } else {
            s.pay_rate.percent_string()
        };
        widgets::kv(ui, &p, "支付率", &rate_text);
        if let Some(d) = d {
            if !tight {
                widgets::kv(ui, &p, "签订日期", &d.sign_date);
                widgets::kv(
                    ui,
                    &p,
                    "交工日期",
                    d.delivery_date.as_deref().unwrap_or("—"),
                );
                // R-66：合同类型 / 开标时间 / 招标代理三行已移除 ——
                // 侧栏改为常驻后高度更紧张，这三项在「合同信息修改」里都能看到
                let bid_label = d
                    .bid_method
                    .as_ref()
                    .map(|b| b.label())
                    .unwrap_or_else(|| "—".to_string());
                widgets::kv(ui, &p, "招标方式", &bid_label);
                widgets::kv(
                    ui,
                    &p,
                    "项目负责人",
                    d.project_leader.as_deref().unwrap_or("—"),
                );
                widgets::kv(ui, &p, "联系方式", d.contact_info.as_deref().unwrap_or("—"));
            }
        }

        if s.is_inverted {
            ui.add_space(6.0);
            widgets::soft_box(&p, p.danger_soft).show(ui, |ui| {
                ui.label(
                    RichText::new("已超有效计价基准：累计已付高于基准，请核对审计价或拨款单。")
                        .size(11.5)
                        .color(p.danger),
                );
            });
        }

        ui.add_space(6.0);
        widgets::divider(ui, &p);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(4.0, 4.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            let att_label = format!("附件 {}/4", app.att_count.clamp(0, 4));
            let att_btn = widgets::ghost_button_sm(ui, &p, &att_label);
            let att_btn = if app.att_count == 0 {
                att_btn.on_hover_text("四类附件均未上传；合同正本为必传项")
            } else {
                att_btn.on_hover_text("查看 / 上传 / 替换 / 另存附件")
            };
            if att_btn.clicked() {
                *action = Some(Act::Attachments);
            }
            if widgets::ghost_button_sm(ui, &p, "登记审计价").clicked() {
                *action = Some(Act::Audit);
            }
            if widgets::ghost_button_sm(ui, &p, "项目交工").clicked() {
                *action = Some(Act::Delivery);
            }
            // R-50：第 5 个按钮，与上面 4 个同一行（详情卡已加宽到 344 容纳）。
            // 浏览用户不显示：写类入口对只读身份没有意义，露出来只会点一次报一次无权限。
            if app.can_write()
                && widgets::ghost_button_sm(ui, &p, "信息修改")
                    .on_hover_text("修改合同名称、单位、工期等主档信息；金额与编号不在此改")
                    .clicked()
            {
                *action = Some(Act::EditContract);
            }
            let pay = widgets::ghost_button_sm_on(ui, &p, "去拨款", can_pay);
            let pay = if can_pay {
                pay
            } else {
                pay.on_hover_text("草稿 / 作废合同不可拨款，先点标题行的「设为生效」")
            };
            if pay.clicked() && can_pay {
                *action = Some(Act::ToPayments(s.contract_id.clone()));
            }
        });
        if is_draft {
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "草稿不可登记拨款；点标题行「设为生效」即可，生效要求合同附件已在位。",
                )
                .size(10.5)
                .color(p.text_muted),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_core::ledger::ContractSummary;
    use ctp_core::rate::Rate;

    fn summary(
        id: &str,
        no: &str,
        name: &str,
        contractor: &str,
        amount_cent: i64,
    ) -> ContractSummary {
        ContractSummary {
            contract_id: id.into(),
            project_id: "P".into(),
            contract_no: no.into(),
            contract_name: name.into(),
            status: "PERFORMING".into(),
            contractor_name: contractor.into(),
            contract_type: "CONSTRUCT".into(),
            contract_category_code: "GC".into(),
            category_code: None,
            category_name: None,
            total_amount: Money::from_cent(amount_cent),
            audit_price: None,
            is_audited: false,
            base_amount: Money::from_cent(amount_cent),
            audit_reduction: Money::ZERO,
            paid_amount: Money::ZERO,
            inflight_amount: Money::ZERO,
            occupied_amount: Money::ZERO,
            available_amount: Money::from_cent(amount_cent),
            pay_rate: Rate::from_bp(0),
            is_inverted: false,
            planned_end: None,
            duration_days: 0,
            delivery_date: None,
        }
    }

    /// R-59：单位面板只列同一承包单位的其他合同，当前行必须排除。
    #[test]
    fn 承包单位面板排除当前行且只取同单位() {
        let all = [
            summary("C1", "HT-001", "道路工程一标", "某建工集团", 1_000),
            summary("C2", "HT-002", "桥梁加固", "某建工集团", 2_000),
            summary("C3", "HT-003", "沥青采购", "某建材公司", 3_000),
            summary("C4", "HT-004", "管网改造", "某建工集团", 4_000),
        ];
        let others = other_contracts_of(&all, "某建工集团", "C1");
        let ids: Vec<&str> = others.iter().map(|o| o.contract_id.as_str()).collect();
        assert_eq!(ids, vec!["C2", "C4"], "同单位其他合同应为 C2、C4");
        assert!(others.iter().all(|o| o.contractor_name == "某建工集团"));
        assert!(
            others.iter().all(|o| o.contract_id != "C1"),
            "当前行不得重复出现"
        );

        // 唯一的合同：面板给空态，而不是把当前行当作「其他合同」
        let only = [summary("C9", "HT-009", "独立合同", "某设计院", 500)];
        assert!(other_contracts_of(&only, "某设计院", "C9").is_empty());
        // 单位名不匹配时不串台
        assert!(other_contracts_of(&all, "某设备公司", "C1").is_empty());
    }

    /// R-63：合计合同金额 = 本合同 + 同单位其他合同。份数含本合同，金额也必须含，
    /// 否则同一块面板里两个数各自一套口径；没有其他合同时退化为本合同金额而不是 0。
    #[test]
    fn 合计合同金额包含本合同() {
        let all = [
            summary("C1", "HT-001", "道路工程一标", "某建工集团", 1_000),
            summary("C2", "HT-002", "桥梁加固", "某建工集团", 2_000),
            summary("C4", "HT-004", "管网改造", "某建工集团", 4_000),
        ];
        let others = other_contracts_of(&all, "某建工集团", "C1");
        assert_eq!(others.len(), 2, "面板只列其他两份");
        // 本合同 1000 + 其他 2000 + 4000
        assert_eq!(
            contractor_total_cent(all[0].total_amount.cent(), &others),
            7_000
        );

        // 空态：合计退化为本合同金额，不能因为「没有其他合同」而变成 0 或消失
        let only = [summary("C9", "HT-009", "独立合同", "某设计院", 500)];
        let none = other_contracts_of(&only, "某设计院", "C9");
        assert!(none.is_empty());
        assert_eq!(
            contractor_total_cent(only[0].total_amount.cent(), &none),
            500
        );

        // 饱和相加：极端金额不 panic、不翻转成负数
        assert_eq!(contractor_total_cent(i64::MAX, &[&only[0]]), i64::MAX);
    }

    /// R-61：点空白收起与点展开入口在同一帧发生时，展开优先 ——
    /// 不经过「先全部收起、再展开」的中间态，否则会多一次状态写入，
    /// 在帧首执行时还会让布局先塌一次（本次故障的根因）。
    #[test]
    fn 空白点击让位于展开意图() {
        assert!(
            should_collapse_panels(true, false),
            "点空白且无展开意图时必须收起"
        );
        assert!(
            !should_collapse_panels(true, true),
            "同一帧点了展开入口时不得先收起"
        );
        assert!(!should_collapse_panels(false, false), "点在面板内不收起");
        assert!(
            !should_collapse_panels(false, true),
            "点在面板内的展开入口不收起"
        );
    }

    /// R-62：落位补偿 —— 放得下就把行钉回点击时的位置；面板越出下边界就把内容上推，
    /// 刚好放下为止；块比视口还高就让行贴顶。符号写反会把面板推得更远。
    #[test]
    fn 落位补偿把展开块留进视口() {
        // 视口纵向范围 100..=600（高 500）
        // 1) 行在中部、整块（行 300 → 面板底 420）本来就看得见：零补偿
        assert_eq!(settle_delta(300.0, 420.0, 100.0, 600.0, 300.0), 0.0);
        // 2) 行贴着底边、面板越出下边界 80：内容上移 80，面板刚好贴底
        assert_eq!(settle_delta(500.0, 680.0, 100.0, 600.0, 500.0), -80.0);
        // 3) 上方面板被收起，行从 300 被带到 100：内容下移 200 钉回原位
        assert_eq!(settle_delta(100.0, 300.0, 100.0, 600.0, 300.0), 200.0);
        // 4) 块比视口还高（700 > 500）：行贴顶，至少露出面板开头
        assert_eq!(settle_delta(400.0, 1100.0, 100.0, 600.0, 400.0), -300.0);
        // 5) 点完之后用户又滚过一段，锚点跑到视口之上：拉回到顶，不留空白
        assert_eq!(settle_delta(80.0, 200.0, 100.0, 600.0, 40.0), 20.0);
        // 6) 锚点与当前行只差 0.3px：落在容差内，不触发滚动
        assert!(settle_delta(300.3, 420.0, 100.0, 600.0, 300.0).abs() < ANCHOR_EPSILON);
    }

    /// R-61：偏移预修正只在上方面板被收起时才给量 ——
    /// 给它下方的行补偿会把内容凭空推下去，反而制造新的跳动。
    #[test]
    fn 偏移预修正只补被收起面板之上的行() {
        // 展开行在序号 2、点击的是序号 20：目标行会被带上去一个面板高度
        assert_eq!(offset_fix_for(Some(2), Some(20), 186.0), 186.0);
        // 展开行在目标行下方：目标行不动，不补
        assert_eq!(offset_fix_for(Some(20), Some(2), 186.0), 0.0);
        // 同一行换个面板类型：行本身不动，不补
        assert_eq!(offset_fix_for(Some(7), Some(7), 186.0), 0.0);
        // 本来没有展开行（第一次点开）：不补
        assert_eq!(offset_fix_for(None, Some(9), 186.0), 0.0);
        // 点了空白处（收起但没点入口）：不补
        assert_eq!(offset_fix_for(Some(3), None, 186.0), 0.0);
        // 没点到任何入口且没有展开行：不补
        assert_eq!(offset_fix_for(None, None, 186.0), 0.0);
    }

    /// R-64 的排序样本行：四列取值都可控（审计价 `None` = 未审计）
    fn sort_row(
        id: &str,
        amount: i64,
        audit: Option<i64>,
        paid: i64,
        rate_bp: i32,
    ) -> ContractSummary {
        let mut s = summary(id, "HT", "合同", "某建工集团", amount);
        s.audit_price = audit.map(Money::from_cent);
        s.is_audited = audit.is_some();
        s.paid_amount = Money::from_cent(paid);
        s.pay_rate = Rate::from_bp(rate_bp);
        s
    }

    fn order(rows: &[ContractSummary], key: LedgerSortKey, desc: bool) -> Vec<String> {
        let mut v = rows.to_vec();
        sort_summaries(&mut v, key, desc);
        v.iter().map(|s| s.contract_id.clone()).collect()
    }

    /// R-64：四列各按自己的数值排、方向可翻转；审计价未审计（无值）正序倒序都沉底；
    /// 等值行保持筛选原序（稳定排序），反复切换方向不会把等值行的相对位置打乱。
    #[test]
    fn 台账排序四列独立且审计价空值沉底() {
        let rows = vec![
            sort_row("A", 300, Some(250), 100, 1_000),
            sort_row("B", 100, None, 300, 9_000),
            sort_row("C", 200, Some(150), 200, 5_000),
            sort_row("D", 400, Some(250), 400, 5_000),
        ];
        // 合同金额
        assert_eq!(
            order(&rows, LedgerSortKey::Amount, true),
            vec!["D", "A", "C", "B"]
        );
        assert_eq!(
            order(&rows, LedgerSortKey::Amount, false),
            vec!["B", "C", "A", "D"]
        );
        // 审计价：B 未审计，两个方向都排在最后；A / D 同为 250 保原序
        assert_eq!(
            order(&rows, LedgerSortKey::Audit, true),
            vec!["A", "D", "C", "B"]
        );
        assert_eq!(
            order(&rows, LedgerSortKey::Audit, false),
            vec!["C", "A", "D", "B"]
        );
        // 累计已付
        assert_eq!(
            order(&rows, LedgerSortKey::Paid, true),
            vec!["D", "B", "C", "A"]
        );
        assert_eq!(
            order(&rows, LedgerSortKey::Paid, false),
            vec!["A", "C", "B", "D"]
        );
        // 支付率：A(10%) 与 D(50%)、C(50%) 等值对，等值保持 C 在 D 前
        assert_eq!(
            order(&rows, LedgerSortKey::Rate, true),
            vec!["B", "C", "D", "A"]
        );
        assert_eq!(
            order(&rows, LedgerSortKey::Rate, false),
            vec!["A", "C", "D", "B"]
        );
    }

    /// R-64：同列再点翻转方向；换列时旧列（含它的方向）被覆盖、新列从倒序开始 ——
    /// 保证「四个字段分别独立排序」不会变成「多个字段叠加」。
    #[test]
    fn 表头点击同列翻转换列重置() {
        // 首次点任意列 → 倒序（金额大的先看）
        assert_eq!(
            next_sort_state(None, LedgerSortKey::Amount),
            (LedgerSortKey::Amount, true)
        );
        assert_eq!(
            next_sort_state(None, LedgerSortKey::Paid),
            (LedgerSortKey::Paid, true)
        );
        // 同列再点 → 翻转
        assert_eq!(
            next_sort_state(Some((LedgerSortKey::Amount, true)), LedgerSortKey::Amount),
            (LedgerSortKey::Amount, false)
        );
        assert_eq!(
            next_sort_state(Some((LedgerSortKey::Amount, false)), LedgerSortKey::Amount),
            (LedgerSortKey::Amount, true)
        );
        // 换列 → 旧列状态不保留，新列重新从倒序开始
        assert_eq!(
            next_sort_state(Some((LedgerSortKey::Amount, false)), LedgerSortKey::Rate),
            (LedgerSortKey::Rate, true)
        );
        assert_eq!(
            next_sort_state(Some((LedgerSortKey::Rate, true)), LedgerSortKey::Audit),
            (LedgerSortKey::Audit, true)
        );
    }

    /// R-65：表头身份只给四个数值列和「合同编号」开口，其余列保持纯文本、不能点。
    #[test]
    fn 表头分类只为四列与编号开口() {
        assert_eq!(head_kind("contract_no"), HeadKind::ResetSort);
        assert_eq!(
            head_kind("total_amount"),
            HeadKind::Sort(LedgerSortKey::Amount)
        );
        assert_eq!(
            head_kind("audit_price"),
            HeadKind::Sort(LedgerSortKey::Audit)
        );
        assert_eq!(
            head_kind("paid_amount"),
            HeadKind::Sort(LedgerSortKey::Paid)
        );
        assert_eq!(head_kind("pay_rate"), HeadKind::Sort(LedgerSortKey::Rate));
        // 名称 / 承包单位 / 交工日期 / 项目状态不是排序入口，也不是重置入口
        for key in [
            "contract_name",
            "contractor_name",
            "delivery_date",
            "project_status",
        ] {
            assert_eq!(head_kind(key), HeadKind::Plain, "{key} 应为纯文本表头");
        }
    }

    /// R-65：标题里的箭头 / ↺ 与高亮都只跟随**当前**排序状态：
    /// 任一时刻最多一列带标记，切换后旧列立即回到无标记的灰字。
    #[test]
    fn 表头标题跟随当前排序状态() {
        // 本列在排：箭头按方向走，且高亮
        let v = head_view(
            "合同金额",
            HeadKind::Sort(LedgerSortKey::Amount),
            Some((LedgerSortKey::Amount, true)),
        );
        assert_eq!(v.label, "合同金额 ↓");
        assert!(v.active);
        let v = head_view(
            "合同金额",
            HeadKind::Sort(LedgerSortKey::Amount),
            Some((LedgerSortKey::Amount, false)),
        );
        assert_eq!(v.label, "合同金额 ↑");
        assert!(v.active);

        // 别的列在排：本列无标记、不高亮
        let v = head_view(
            "审计价",
            HeadKind::Sort(LedgerSortKey::Audit),
            Some((LedgerSortKey::Amount, true)),
        );
        assert_eq!(v.label, "审计价");
        assert!(!v.active);

        // 编号列：排序生效时加 ↺ 并高亮（它是恢复入口），未排序时与普通灰字一致
        let v = head_view(
            "合同编号",
            HeadKind::ResetSort,
            Some((LedgerSortKey::Rate, false)),
        );
        assert_eq!(v.label, "合同编号 ↺");
        assert!(v.active);
        let v = head_view("合同编号", HeadKind::ResetSort, None);
        assert_eq!(v.label, "合同编号");
        assert!(!v.active);
        assert!(!v.tip.is_empty(), "未排序时也应说明当前就是初始排序");

        // 纯文本列永远不突出，也不给点击提示
        let v = head_view(
            "项目状态",
            HeadKind::Plain,
            Some((LedgerSortKey::Amount, true)),
        );
        assert_eq!(v.label, "项目状态");
        assert!(!v.active);
        assert!(v.tip.is_empty());
    }

    /// R-66：右侧栏在常规宽度始终保留，只有窄到装不下 5 列时才让位
    #[test]
    fn 右侧栏在常规宽度始终可见() {
        assert!(detail_visible(1400.0));
        assert!(detail_visible(DETAIL_MIN_W));
        // 差一个像素就不再挤表格：阈值必须是「大于等于」才显示
        assert!(!detail_visible(DETAIL_MIN_W - 1.0));
        assert!(!detail_visible(640.0));
    }

    /// R-66：默认焦点 —— 没选中时落到列表首行；已选中时一律不动，
    /// 哪怕这份合同已经被搜索 / 状态筛选隐藏（用户正在看的东西不该被悄悄换掉）
    #[test]
    fn 未选中时默认聚焦列表首行() {
        let rows = vec![
            summary("C1", "HT-1", "一号", "甲", 100),
            summary("C2", "HT-2", "二号", "乙", 200),
        ];
        assert_eq!(initial_focus(&rows, None), Some("C1".to_string()));
        assert_eq!(initial_focus(&rows, Some("C2")), Some("C2".to_string()));
        assert_eq!(initial_focus(&rows, Some("C9")), Some("C9".to_string()));
        // 列表为空：没有可聚焦的行，交给侧栏的空态
        assert_eq!(initial_focus(&[], None), None);
    }
}
