//! 与设备无关的打印版面。
//!
//! 一张拨款单固定为**两页横向 A4**（297 × 210 mm），分别对应模板里的两张表：
//! 第 1 页《财务支出审核监督表》、第 2 页《工程项目进度款支付申请表》。
//!
//! 坐标系一律用**毫米**、原点在纸张左上角，由后端（`gdi`）换算成设备像素，
//! 因此同一套版面代码既能打到打印机 DC，也能渲染成内存位图做预览，
//! 两者像素级一致，所见即所印。
//!
//! 版面数值不是估的：全部按模板 PDF 的表格线逐线量取（`docs/版式基准_付款打印模板.docx`
//! 导出的 PDF，用 `get_drawings()` 取线段坐标，单位 mm）。改版式时请对照该文件。
//!
//! 三处手工填写位（会计分类名 / 附件张数 / 当前日期）按已确认口径**留空**，
//! 版面里只保留格子，不写任何占位字符。

/// 纸张宽度（mm）——横向 A4。
pub const PAGE_W_MM: f32 = 297.0;
/// 纸张高度（mm）。
pub const PAGE_H_MM: f32 = 210.0;

/// 一页版面固定两页；批量 N 张即 2N 页。
pub const PAGES_PER_VOUCHER: usize = 2;

/// 水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// 文字样式。字号单位 pt（与 Word 一致），后端换算成设备像素。
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub size_pt: f32,
    pub bold: bool,
    pub align: Align,
    /// 在给定矩形内垂直居中（多行也能居中：先测量高度再偏移）。
    pub vcenter: bool,
}

impl TextStyle {
    pub const fn new(size_pt: f32, bold: bool, align: Align, vcenter: bool) -> Self {
        Self {
            size_pt,
            bold,
            align,
            vcenter,
        }
    }

    pub const fn plain(size_pt: f32) -> Self {
        Self::new(size_pt, false, Align::Left, false)
    }
}

/// 绘制后端。所有坐标单位 mm。
///
/// 只要求实现两个原语，其余（垂直居中、自动降字号）在 trait 默认方法里完成，
/// 保证打印与预览两条通道的排版结果完全一致。
pub trait Canvas {
    /// 画一条 0.45pt 细线。
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32);

    /// 画矩形边框（四条细线）。
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32);

    /// 在矩形内绘制文字（不做垂直居中）。
    fn draw_text(&mut self, x: f32, y: f32, w: f32, h: f32, text: &str, style: TextStyle);

    /// 在给定宽度下排版后的文字高度（mm）。
    fn measure_text(&mut self, text: &str, style: TextStyle, w: f32) -> f32;

    /// 矩形内绘制文字，`style.vcenter` 为真时垂直居中。
    fn text(&mut self, x: f32, y: f32, w: f32, h: f32, text: &str, style: TextStyle) {
        if style.vcenter {
            let th = self.measure_text(text, style, w);
            if th > 0.0 && th < h {
                let off = (h - th) / 2.0;
                self.draw_text(x, y + off, w, th, text, style);
                return;
            }
        }
        self.draw_text(x, y, w, h, text, style);
    }
}

/// 一张单子要印出来的全部取值（9 个域已展开成版面直接可用的文本）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Voucher {
    /// 拨款单号（仅用于界面提示与多处定位，不上纸）
    pub payment_no: String,
    /// 项目名称：表 1「支付用途说明」与表 2「项目名称」同值（取合同名称）
    pub project_name: String,
    /// 中标单位（取承包单位）
    pub contractor: String,
    /// 合同价（元，两位小数）
    pub contract_amount: String,
    /// 审计价（元，两位小数；未审定填 0.00）
    pub audit_amount: String,
    /// 本次之前的累计已付（元）
    pub paid_before: String,
    /// 本次金额（元）
    pub this_amount: String,
    /// 本次金额中文大写
    pub this_amount_cn: String,
    /// 本次付款后累计（元）
    pub cumulative: String,
    /// 合同约定的付款条件
    pub terms: String,
}

// ---------------------------------------------------------------- 版面常量

/// 表 1 的横线位置（mm，自页顶起算）。
const S1_ROWS: [f32; 7] = [49.5, 69.25, 88.95, 104.7, 124.45, 148.85, 180.55];
/// 表 1 左右边界。
const S1_LEFT: f32 = 33.35;
const S1_RIGHT: f32 = 274.65;

/// 表 2 的横线位置（mm）。
const S2_ROWS: [f32; 4] = [52.3, 83.85, 139.85, 179.55];
/// 表 2 十条竖线位置（mm），9 列。
const S2_COLS: [f32; 10] = [
    20.85, 53.45, 81.5, 106.35, 137.55, 166.9, 193.35, 223.5, 249.85, 276.65,
];

/// 单元格内文字与边框的留白（mm）。
const PAD: f32 = 1.6;

/// 表 1 正文字号（模板实测 14.5pt）。
const S1_PT: f32 = 14.5;

// ---------------------------------------------------------------- 入口

/// 画第 `page` 页（0 起）。
pub fn paint(c: &mut dyn Canvas, v: &Voucher, page: usize) {
    if page % PAGES_PER_VOUCHER == 0 {
        sheet_supervision(c, v);
    } else {
        sheet_application(c, v);
    }
}

// ---------------------------------------------------------------- 第 1 页

fn sheet_supervision(c: &mut dyn Canvas, v: &Voucher) {
    // 标题（模板 16pt 加粗、居中于表格中线）
    c.text(
        S1_LEFT,
        23.2,
        S1_RIGHT - S1_LEFT,
        10.0,
        "财务支出审核监督表",
        TextStyle::new(16.0, true, Align::Center, true),
    );

    // 部门行：「部门: 区交通运输局」左、「年 月 日」右（日期留空待手填）
    c.text(
        S1_LEFT,
        40.6,
        140.0,
        8.0,
        "部门: 区交通运输局",
        TextStyle::new(16.0, false, Align::Left, true),
    );
    c.text(
        163.0,
        40.6,
        90.0,
        8.0,
        "年　　　月　　　日",
        TextStyle::new(14.5, false, Align::Right, true),
    );

    // ---- 表格：6 行，每行分格数不同（按模板合并关系逐行定义）
    let r = S1_ROWS;

    // 行 1：支付内容（会计科目手填，留空）
    cell(c, S1_LEFT, r[0], 89.75 - S1_LEFT, r[1] - r[0], "支付内容\n(按照会计科目)：", Align::Center);
    // 行 2：支付金额 —— 大写（89.75..216.15）/ 小写（216.15..274.65）
    cell(c, S1_LEFT, r[1], 89.75 - S1_LEFT, r[2] - r[1], "支付金额：", Align::Center);
    cell_fit(
        c,
        89.75,
        r[1],
        216.15 - 89.75,
        r[2] - r[1],
        &format!("大写:{}", v.this_amount_cn),
        Align::Left,
    );
    cell_fit(
        c,
        216.15,
        r[1],
        274.65 - 216.15,
        r[2] - r[1],
        &format!("小写:{}", v.this_amount),
        Align::Right,
    );
    // 行 3：票据 / 附件张数（张数留空手填，位置按模板的 tab 停位）
    cell(c, S1_LEFT, r[2], 135.7 - S1_LEFT, r[3] - r[2], "票　　据:", Align::Left);
    c.text(
        117.0,
        r[2],
        10.0,
        r[3] - r[2],
        "张",
        TextStyle::new(S1_PT, false, Align::Left, true),
    );
    cell(c, 135.7, r[2], S1_RIGHT - 135.7, r[3] - r[2], "附件:", Align::Left);
    c.text(
        190.0,
        r[2],
        10.0,
        r[3] - r[2],
        "张",
        TextStyle::new(S1_PT, false, Align::Left, true),
    );
    // 行 4：经办人签名（33.35..110.15）/ 支付用途说明（110.15..274.65）
    cell(c, S1_LEFT, r[3], 110.15 - S1_LEFT, r[4] - r[3], "经办人签名:", Align::Left);
    cell_fit(
        c,
        110.15,
        r[3],
        S1_RIGHT - 110.15,
        r[4] - r[3],
        &format!("支付用途说明:付 {} 工程款", v.project_name),
        Align::Left,
    );
    // 行 5：三个审核栏（各带一个手签空格）
    let x5 = [33.35, 63.5, 110.15, 135.7, 187.45, 216.15, 274.65];
    for i in 0..3 {
        // 标签只占左半，右半留空供手签
        let (lbl, a, label_w) = match i {
            0 => ("部门负责人\n意见:", x5[0], x5[1] - x5[0]),
            1 => ("财务负责人审核：", x5[2], x5[3] - x5[2]),
            _ => ("办公室负责人审核：", x5[4], x5[5] - x5[4]),
        };
        cell(c, a, r[4], label_w, r[5] - r[4], lbl, Align::Center);
    }
    // 行 6：分管领导审批 / 单位负责人批准（竖线在 33.35 / 63.5 / 135.7 / 187.45 / 274.65）
    cell(c, x5[0], r[5], x5[1] - x5[0], r[6] - r[5], "分管领导\n审批：", Align::Center);
    cell(c, 135.7, r[5], 187.45 - 135.7, r[6] - r[5], "单位负责人\n批准：", Align::Center);

    // ---- 网格线（按每行实际分格画竖线，再统一画横线）
    let row_cols: [&[f32]; 6] = [
        &[S1_LEFT, 89.75, S1_RIGHT],
        &[S1_LEFT, 89.75, 216.15, S1_RIGHT],
        &[S1_LEFT, 135.7, S1_RIGHT],
        &[S1_LEFT, 110.15, S1_RIGHT],
        &x5,
        &[x5[0], x5[1], x5[3], x5[5], x5[6]],
    ];
    for (i, cols) in row_cols.iter().enumerate() {
        for x in cols.iter() {
            c.line(*x, r[i], *x, r[i + 1]);
        }
    }
    for y in r.iter() {
        c.line(S1_LEFT, *y, S1_RIGHT, *y);
    }
}

// ---------------------------------------------------------------- 第 2 页

fn sheet_application(c: &mut dyn Canvas, v: &Voucher) {
    let left = S2_COLS[0];
    let right = S2_COLS[9];

    c.text(
        left,
        23.2,
        right - left,
        10.0,
        "工程项目进度款支付申请表",
        TextStyle::new(16.0, true, Align::Center, true),
    );
    // 「年 月 日」在左、「单位：元」在右，二者同一行
    c.text(
        38.0,
        42.2,
        60.0,
        7.0,
        "年　　　月　　　日",
        TextStyle::new(11.0, false, Align::Left, true),
    );
    c.text(
        right - 70.0,
        42.2,
        70.0,
        7.0,
        "单位：元",
        TextStyle::new(11.0, false, Align::Right, true),
    );

    let r = S2_ROWS;
    // 断行位置手工给定，避免自动折行把双字词拆开、末行只剩一个字。
    // 长条款列另给较小基准字号，使整格保持在四行以内。
    let headers: [(&str, f32); 9] = [
        ("项目名称", 9.0),
        ("施工单位", 9.0),
        ("合同价", 9.0),
        ("审计价\n（如已审计填报）", 8.0),
        ("合同履行情况\n(已完成工程量*%/\n已交工验收/\n已交工验收满*年)", 7.5),
        ("目前已累计\n支付金额", 9.0),
        ("本期申请\n支付金额", 9.0),
        ("本期末累计\n支付金额", 9.0),
        ("备注", 9.0),
    ];
    let values = [
        v.project_name.clone(),
        v.contractor.clone(),
        v.contract_amount.clone(),
        v.audit_amount.clone(),
        v.terms.clone(),
        v.paid_before.clone(),
        v.this_amount.clone(),
        v.cumulative.clone(),
        String::new(),
    ];

    // 表头行 / 取值行：9 列全分
    for i in 0..9 {
        let (x, w) = (S2_COLS[i], S2_COLS[i + 1] - S2_COLS[i]);
        fit_in(c, x, r[0], w, r[1] - r[0], headers[i].0, headers[i].1, 6.0, Align::Center);
        if !values[i].is_empty() {
            fit_in(c, x, r[1], w, r[2] - r[1], &values[i], 10.5, 5.5, Align::Center);
        }
    }

    // 签字行：跨列区间必须与下面网格线画出的合并格一致。
    let sign = [
        (S2_COLS[0], S2_COLS[1], "经办人："),
        (S2_COLS[1], S2_COLS[3], "项目牵头部门\n负责人："),
        (S2_COLS[4], S2_COLS[6], "项目牵头部门\n分管领导意见："),
    ];
    for (a, b, txt) in sign {
        fit_in(c, a, r[2], b - a, r[3] - r[2], txt, 9.0, 7.0, Align::Center);
    }

    // 网格线：表头/取值行保持全列，签字行按合并后的分格
    for i in 0..10 {
        c.line(S2_COLS[i], r[0], S2_COLS[i], r[2]);
    }
    for x in [S2_COLS[0], S2_COLS[1], S2_COLS[3], S2_COLS[4], S2_COLS[6], S2_COLS[7], S2_COLS[9]] {
        c.line(x, r[2], x, r[3]);
    }
    for y in r.iter() {
        c.line(left, *y, right, *y);
    }
}

// ---------------------------------------------------------------- 工具

/// 带边框的单元格：先画文本，边框由调用方统一画。
fn cell(c: &mut dyn Canvas, x: f32, y: f32, w: f32, h: f32, text: &str, align: Align) {
    c.text(
        x + PAD,
        y,
        w - PAD * 2.0,
        h,
        text,
        TextStyle::new(S1_PT, false, align, true),
    );
}

/// 表 1 的取值格：左对齐、垂直居中，超宽时略降字号。
fn cell_fit(c: &mut dyn Canvas, x: f32, y: f32, w: f32, h: f32, text: &str, align: Align) {
    let mut size = S1_PT;
    while size > 8.0 {
        let st = TextStyle::new(size, false, align, true);
        if c.measure_text(text, st, w - PAD * 2.0) <= h - 1.0 {
            break;
        }
        size -= 0.5;
    }
    c.text(
        x + PAD,
        y,
        w - PAD * 2.0,
        h,
        text,
        TextStyle::new(size, false, align, true),
    );
}

/// 表 2 的通用单元格：居中，按格宽格高自动降字号，避免长条款溢出格线。
fn fit_in(
    c: &mut dyn Canvas,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: &str,
    base_pt: f32,
    min_pt: f32,
    align: Align,
) {
    let mut size = base_pt;
    while size > min_pt {
        let st = TextStyle::new(size, false, align, true);
        if c.measure_text(text, st, w - PAD * 2.0) <= h - 2.0 {
            break;
        }
        size -= 0.5;
    }
    c.text(
        x + PAD,
        y,
        w - PAD * 2.0,
        h,
        text,
        TextStyle::new(size, false, align, true),
    );
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录所有绘制调用的假画布，用来断言版面结构。
    #[derive(Default)]
    struct Rec {
        lines: usize,
        texts: Vec<String>,
    }

    impl Canvas for Rec {
        fn line(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32) {
            self.lines += 1;
        }
        fn rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32) {
            self.lines += 4;
        }
        fn draw_text(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, s: &str, _st: TextStyle) {
            self.texts.push(s.to_string());
        }
        fn measure_text(&mut self, text: &str, _st: TextStyle, _w: f32) -> f32 {
            // 每 20 字一行，行高按字号估算（足够驱动降字号逻辑）
            let lines = (text.chars().count() as f32 / 20.0).ceil().max(1.0);
            lines * 4.0
        }
    }

    fn sample() -> Voucher {
        Voucher {
            payment_no: "FK-2026-001".into(),
            project_name: "XX 公路改建工程".into(),
            contractor: "YY 路桥有限公司".into(),
            contract_amount: "1000.00".into(),
            audit_amount: "0.00".into(),
            paid_before: "0.00".into(),
            this_amount: "600.00".into(),
            this_amount_cn: "陆佰圆整".into(),
            cumulative: "600.00".into(),
            terms: "已完成工程量 60%".into(),
        }
    }

    #[test]
    fn 两页各自的标题与关键字段都上纸() {
        let mut first = Rec::default();
        paint(&mut first, &sample(), 0);
        assert!(first.texts.iter().any(|t| t == "财务支出审核监督表"));
        assert!(first.texts.iter().any(|t| t.contains("大写:陆佰圆整")));
        assert!(first.texts.iter().any(|t| t.contains("小写:600.00")));
        assert!(first
            .texts
            .iter()
            .any(|t| t.contains("支付用途说明:付 XX 公路改建工程 工程款")));

        let mut second = Rec::default();
        paint(&mut second, &sample(), 1);
        assert!(second.texts.iter().any(|t| t == "工程项目进度款支付申请表"));
        assert!(second.texts.iter().any(|t| t == "XX 公路改建工程"));
        assert!(second.texts.iter().any(|t| t == "1000.00"));
        assert!(second.texts.iter().any(|t| t == "600.00"));
    }

    #[test]
    fn 三处手工位保持空白不写占位符() {
        let mut rec = Rec::default();
        paint(&mut rec, &sample(), 0);
        // 支付内容 / 票据张数 / 附件张数 只有标签，没有任何占位数字
        for t in &rec.texts {
            assert!(!t.contains("«"), "版面不应出现占位符：{t}");
            assert!(!t.contains("MERGEFIELD"), "版面不应出现域代码：{t}");
        }
    }

    #[test]
    fn 网格线数量固定不随数据变化() {
        let mut a = Rec::default();
        paint(&mut a, &sample(), 0);
        let mut b = Rec::default();
        paint(&mut b, &sample(), 1);
        // 表 1：6 行的分格竖线 3+4+3+3+7+5 = 25 条 + 7 条横线
        assert_eq!(a.lines, 25 + 7);
        // 表 2：10 列贯穿前两行 + 签字行 7 条 + 4 条横线
        assert_eq!(b.lines, 10 + 7 + 4);
    }

    #[test]
    fn 页码范围之外按两页循环() {
        let mut rec = Rec::default();
        paint(&mut rec, &sample(), 2);
        assert!(rec.texts.iter().any(|t| t == "财务支出审核监督表"));
    }
}
