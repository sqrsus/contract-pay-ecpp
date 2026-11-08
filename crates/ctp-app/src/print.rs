//! 付款打印：**字段口径 + 版面数据**。
//!
//! 本模块只负责两件事，不碰任何图形接口：
//! 1. 把模板里的 9 个域与系统字段的映射固化成 `PAYMENT_TEMPLATE` 描述符；
//! 2. 把一张拨款单解析成 `Voucher`（版面直接可用的文本），供 `paper` 排版。
//!
//! 输出通道（R-28 起）：**不再经过 Word / PDF**。`paper` 用毫米坐标描述两页版面，
//! `gdi` 把同一份版面分别画到打印机 DC（弹系统打印对话框）与内存位图（界面预览），
//! 二者像素一致。原 Word 模板填充、docx→PDF 两条链路已整体移除。
//!
//! 模板实物解析结论（2026-11-05 复核，改版式时仍需遵守）：11 处域 / 9 个唯一域名、
//! 两张表原在同一 docx（现按该 docx 的 PDF 逐线复刻为 `paper` 的两页），
//! 无动态明细行；4 个域的 `«…»` 在 XML 里被拆成 3 个 run。
//! 版式基准文件保留在 `docs/版式基准_付款打印模板.docx`。

use crate::paper::{self, Voucher};
use ctp_core::date::now_iso;
use ctp_core::money::Money;
use ctp_core::payment::PaymentStatus;
use ctp_store::{repo, StoreError};
use rusqlite::Connection;
use std::fmt;

// ---------------------------------------------------------------- 模板决策常量

/// 原始模板类型。
///
/// R-28 起输出已改为 `paper` + GDI 直接排版，不再填充这份模板；
/// 保留该枚举只是为了存档「9 个域当初来自哪种模板」，并让描述符字段保持稳定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Word,
    Excel,
}

impl TemplateKind {
    pub fn code(self) -> &'static str {
        match self {
            TemplateKind::Word => "DOCX",
            TemplateKind::Excel => "XLSX",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TemplateKind::Word => "Word 邮件合并域模板（版式基准）",
            TemplateKind::Excel => "Excel 单元格模板",
        }
    }
}

impl fmt::Display for TemplateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 域的最终处理方式。已确认走 (a)：整个 MERGEFIELD 替换成纯文本值，
/// 打印件不会被 Word「更新域」冲掉。（b）保留域代码、（c）只导出数据源暂不实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillStrategy {
    ReplaceFieldWithText,
    KeepFieldCode,
    MergeDataSourceOnly,
}

/// 字段缺失时的处置。已确认「留空」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingPolicy {
    Blank,
    Dash,
    Reject,
}

/// 一次打印的输出范围。已确认「两张表同在一个 docx 一起输出」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputScope {
    BothTablesOneFile,
    SplitFiles,
}

/// 域取值来源（业务语义，不写 SQL：具体取数在 `resolve()` 一处落地）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSource {
    /// 合同名称（`ct_contract.contract_name`）——已确认不是项目名称
    ContractName,
    /// 承包单位名称——已确认「中标单位」取承包单位，不取发包人
    ContractorName,
    /// 合同价（原始签约金额，元）
    ContractOriginalAmount,
    /// 审计价，未审定为 0
    AuditPriceOrZero,
    /// 本次之前的累计已付
    PaidBeforeThis,
    /// 本次金额（元）
    ThisPaymentAmount,
    /// 本次金额的中文大写
    ThisPaymentUppercase,
    /// 已付 + 本次
    CumulativeAfterThis,
    /// 付款条款
    PaymentTerms,
}

impl FieldSource {
    pub fn label(self) -> &'static str {
        match self {
            FieldSource::ContractName => "合同名称",
            FieldSource::ContractorName => "承包单位",
            FieldSource::ContractOriginalAmount => "合同价（元）",
            FieldSource::AuditPriceOrZero => "审计价（元），未审定填 0",
            FieldSource::PaidBeforeThis => "本次之前的累计已付（元）",
            FieldSource::ThisPaymentAmount => "本次金额（元）",
            FieldSource::ThisPaymentUppercase => "本次金额中文大写",
            FieldSource::CumulativeAfterThis => "已付 + 本次（元）",
            FieldSource::PaymentTerms => "付款条款",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValueKind {
    Text,
    /// 固定两位小数的「元」字符串（模板域带 `\# "0.00"` 开关）
    AmountYuan,
    AmountUppercase,
}

/// 一个 Word 合并域 → 系统字段的绑定。
#[derive(Debug, Clone, Copy)]
pub struct FieldBinding {
    /// `w:instrText` 里的原始域名（必须与模板逐字一致）
    pub merge_field: &'static str,
    /// 界面预览用的中文名
    pub label: &'static str,
    pub source: FieldSource,
    pub kind: FieldValueKind,
    /// 需要人工留意的口径说明
    pub note: &'static str,
}

/// 模板描述符：把已确认的交付口径一并固化，避免后期实现时反复追问。
#[derive(Debug)]
pub struct TemplateDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: TemplateKind,
    /// 备选模板类型（第二 adapter）
    pub alt_kind: Option<TemplateKind>,
    /// 模板文件位置提示；本期不读取模板文件
    pub template_file_hint: &'static str,
    pub fill_strategy: FillStrategy,
    pub missing_policy: MissingPolicy,
    pub output_scope: OutputScope,
    /// 没有合并域、留待手工填写或签字的位置
    pub manual_blank_fields: &'static [&'static str],
    pub fields: &'static [FieldBinding],
}

/// 《付款打印模板.docx》的 9 个唯一合并域（11 处出现）。顺序按模板内首次出现顺序。
pub static PAYMENT_FIELDS: &[FieldBinding] = &[
    FieldBinding {
        merge_field: "本次付款数大写",
        label: "本次付款（大写）",
        source: FieldSource::ThisPaymentUppercase,
        kind: FieldValueKind::AmountUppercase,
        note: "由本次金额转换为财务中文大写（零壹贰…圆角分整）；系统原无此能力，本轮新增 ctp-core/amount_cn.rs",
    },
    FieldBinding {
        merge_field: "本次付款小写",
        label: "本次付款（小写）",
        source: FieldSource::ThisPaymentAmount,
        kind: FieldValueKind::AmountYuan,
        note: "表1「小写」与表2「本期申请支付金额」两处同值；按本次实付映射（系统无申请/审批模块）",
    },
    FieldBinding {
        merge_field: "项目名称",
        label: "项目名称",
        source: FieldSource::ContractName,
        kind: FieldValueKind::Text,
        note: "已确认取「合同名称」，不是 prj_project.project_name；表1 支付用途说明与表2 项目名称两处同值",
    },
    FieldBinding {
        merge_field: "中标单位",
        label: "中标单位",
        source: FieldSource::ContractorName,
        kind: FieldValueKind::Text,
        note: "已确认取承包单位；模板表头写「施工单位」，与发包人（甲方）无关",
    },
    FieldBinding {
        merge_field: "合同改成元",
        label: "合同价",
        source: FieldSource::ContractOriginalAmount,
        kind: FieldValueKind::AmountYuan,
        note: "取合同原价（元、两位小数）；「改成元」是模板编辑备注，不是字段语义",
    },
    FieldBinding {
        merge_field: "审计价改成元",
        label: "审计价",
        source: FieldSource::AuditPriceOrZero,
        kind: FieldValueKind::AmountYuan,
        note: "未审定填 0；为与域的 \\# \"0.00\" 数字开关一致，输出 0.00",
    },
    FieldBinding {
        merge_field: "合同约定_付款条件（保持与书面合同一致）",
        label: "合同约定 / 付款条件",
        source: FieldSource::PaymentTerms,
        kind: FieldValueKind::Text,
        note: "已确认取 ct_contract.payment_terms；模板表头写「合同履行情况」，与域名不符，按域名口径填付款条款。该域在 XML 里被拆成 3 个 instrText run，后期填充必须按域边界处理",
    },
    FieldBinding {
        merge_field: "已付款改成元",
        label: "累计已付",
        source: FieldSource::PaidBeforeThis,
        kind: FieldValueKind::AmountYuan,
        note: "本次之前的累计已付；与 V2 窗口同序：(pay_date, payment_no) 严格小于本单，仅计 PAID",
    },
    FieldBinding {
        merge_field: "本次付款后累计",
        label: "本次付款后累计",
        source: FieldSource::CumulativeAfterThis,
        kind: FieldValueKind::AmountYuan,
        note: "已付 + 本次",
    },
];

/// 三个无合并域、保持空白的位置（已确认留空，不自动填会计分类名 / 附件张数 / 当前日期）。
pub static MANUAL_BLANK_FIELDS: &[&str] = &[
    "支付内容（会计分类名）",
    "附件张数",
    "当前日期（部门 / 年月日）",
];

/// 付款打印的版式描述符：9 个域的来源与口径都固定在这里。
///
/// R-28 起这张单据由 `paper` 直接排版，本描述符只承担两件事：
/// 声明每个域「取哪个系统字段」，以及存档版式基准文件的位置。
pub static PAYMENT_TEMPLATE: TemplateDescriptor = TemplateDescriptor {
    id: "payment_voucher_v1",
    name: "付款打印（财务支出审核监督表 + 工程项目进度款支付申请表）",
    kind: TemplateKind::Word,
    alt_kind: Some(TemplateKind::Excel),
    template_file_hint: "docs/版式基准_付款打印模板.docx",
    fill_strategy: FillStrategy::ReplaceFieldWithText,
    missing_policy: MissingPolicy::Blank,
    output_scope: OutputScope::BothTablesOneFile,
    manual_blank_fields: MANUAL_BLANK_FIELDS,
    fields: PAYMENT_FIELDS,
};

/// 模板注册表（本期只有一份；后期加模板只往这里追加）。
pub static TEMPLATES: &[&TemplateDescriptor] = &[&PAYMENT_TEMPLATE];

pub fn find_template(id: &str) -> Option<&'static TemplateDescriptor> {
    TEMPLATES.iter().copied().find(|t| t.id == id)
}

// ---------------------------------------------------------------- 数据层

/// 已解析的一个域取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedField {
    pub merge_field: &'static str,
    pub label: &'static str,
    pub value: String,
    pub source: &'static str,
    pub note: &'static str,
}

/// 一张拨款单的打印数据（不含任何文件 I/O）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentPrintData {
    pub payment_no: String,
    pub contract_no: String,
    pub fields: Vec<ResolvedField>,
    pub generated_at: String,
}

impl PaymentPrintData {
    /// 按原始合并域名取值，供后期 adapter 使用。
    pub fn get(&self, merge_field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.merge_field == merge_field)
            .map(|f| f.value.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    #[error("字段缺失且当前策略为拒绝生成：{0}")]
    MissingField(&'static str),

    #[error("该拨款单已作废，不允许打印：{0}")]
    Voided(String),

    #[error("没有选中任何拨款单")]
    NothingSelected,

    #[error("打印失败：{0}")]
    Print(String),

    #[error("已取消打印")]
    Cancelled,

    #[error("{0}")]
    Store(#[from] StoreError),
}

fn apply_policy(
    value: Option<String>,
    policy: MissingPolicy,
    field: &'static str,
) -> Result<String, PrintError> {
    match value {
        Some(s) if !s.is_empty() => Ok(s),
        _ => match policy {
            MissingPolicy::Blank => Ok(String::new()),
            MissingPolicy::Dash => Ok("—".to_string()),
            MissingPolicy::Reject => Err(PrintError::MissingField(field)),
        },
    }
}

/// 解析一张拨款单的 9 个合并域取值。
///
/// 取数全部走 `ctp-store` 的既有仓储，不另写业务口径：
/// * 合同名 / 合同价 / 审计价 / 付款条款 → `repo::get_contract`
/// * 承包单位名 → `repo::summaries`（V1 视图口径）
/// * 本次之前的累计已付 → `repo::paid_before_payment`（与 V2 窗口同序）
pub fn resolve(conn: &Connection, payment_id: &str) -> Result<PaymentPrintData, PrintError> {
    let pm = repo::get_payment(conn, payment_id)?;
    let contract = repo::get_contract(conn, &pm.contract_id).ok();
    let contractor_name = repo::summaries(conn)?
        .into_iter()
        .find(|s| s.contract_id == pm.contract_id)
        .map(|s| s.contractor_name);
    let paid_before =
        repo::paid_before_payment(conn, &pm.contract_id, &pm.pay_date, &pm.payment_no)?;
    let after = Money::from_cent(paid_before.cent().saturating_add(pm.amount_cent.cent()));

    let d = &PAYMENT_TEMPLATE;
    let amount_yuan = pm.amount_cent.fixed_yuan();
    let uppercase = pm.amount_cent.rmb_uppercase();

    let mut fields = Vec::with_capacity(d.fields.len());
    for b in d.fields {
        let raw: Option<String> = match b.source {
            FieldSource::ContractName => contract.as_ref().map(|c| c.contract_name.clone()),
            FieldSource::ContractorName => contractor_name.clone(),
            FieldSource::ContractOriginalAmount => contract
                .as_ref()
                .map(|c| c.original_amount_cent.fixed_yuan()),
            // 审计价未审定按 0；输出 0.00 与模板 \# "0.00" 开关一致
            FieldSource::AuditPriceOrZero => Some(
                contract
                    .as_ref()
                    .and_then(|c| c.audit_price())
                    .unwrap_or(Money::ZERO)
                    .fixed_yuan(),
            ),
            FieldSource::PaidBeforeThis => Some(paid_before.fixed_yuan()),
            FieldSource::ThisPaymentAmount => Some(amount_yuan.clone()),
            FieldSource::ThisPaymentUppercase => Some(uppercase.clone()),
            FieldSource::CumulativeAfterThis => Some(after.fixed_yuan()),
            FieldSource::PaymentTerms => contract.as_ref().map(|c| c.payment_terms.clone()),
        };
        let value = apply_policy(raw, d.missing_policy, b.merge_field)?;
        fields.push(ResolvedField {
            merge_field: b.merge_field,
            label: b.label,
            value,
            source: b.source.label(),
            note: b.note,
        });
    }

    Ok(PaymentPrintData {
        payment_no: pm.payment_no.clone(),
        contract_no: contract
            .as_ref()
            .map(|c| c.contract_no.clone())
            .unwrap_or_default(),
        fields,
        generated_at: now_iso(),
    })
}

// ---------------------------------------------------------------- 版面数据

/// 把解析结果折叠成版面直接可用的文本。
///
/// 9 个域里有两处「一域多贴」：表 1 的支付用途说明与表 2 的项目名称同值。
/// 版面按语义取用（`project_name` 一处填两处），不在这里重复展开。
pub fn to_voucher(data: &PaymentPrintData) -> Voucher {
    let get = |name: &str| data.get(name).unwrap_or("").to_string();
    Voucher {
        payment_no: data.payment_no.clone(),
        project_name: get("项目名称"),
        contractor: get("中标单位"),
        contract_amount: get("合同改成元"),
        audit_amount: get("审计价改成元"),
        paid_before: get("已付款改成元"),
        this_amount: get("本次付款小写"),
        this_amount_cn: get("本次付款数大写"),
        cumulative: get("本次付款后累计"),
        terms: get("合同约定_付款条件（保持与书面合同一致）"),
    }
}

/// 解析一批拨款单，供打印与预览使用。
///
/// * 作废单一律拒绝（已确认「作废不允许打印」）；
/// * 顺序按传入顺序（界面按列表顺序传，勾选先后不影响成品次序）。
pub fn vouchers(conn: &Connection, payment_ids: &[String]) -> Result<Vec<Voucher>, PrintError> {
    if payment_ids.is_empty() {
        return Err(PrintError::NothingSelected);
    }
    let mut out = Vec::with_capacity(payment_ids.len());
    for id in payment_ids {
        let pm = repo::get_payment(conn, id)?;
        if pm.status != PaymentStatus::Paid {
            return Err(PrintError::Voided(pm.payment_no));
        }
        out.push(to_voucher(&resolve(conn, id)?));
    }
    Ok(out)
}

/// 这批单据总共印几页。
pub fn page_count(count: usize) -> usize {
    count * paper::PAGES_PER_VOUCHER
}

/// 打印件名称：单张带单号，多张带首单号与张数。
///
/// 只用于打印作业名与界面提示（不再落盘，所以没有扩展名）。
pub fn doc_name(vs: &[Voucher]) -> String {
    match vs {
        [] => "拨款单".to_string(),
        [one] => format!("拨款单 {}", one.payment_no),
        [first, ..] => format!("拨款单 {} 等 {} 张", first.payment_no, vs.len()),
    }
}

/// 一页的预览位图（RGBA8，逐行自上而下）。
///
/// 由打印后端按同一份 `paper` 版面渲染，供界面显示；与出纸结果像素一致。
pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 描述符固化了九域与已确认口径() {
        let d = &PAYMENT_TEMPLATE;
        assert_eq!(d.fields.len(), 9);
        assert_eq!(d.alt_kind, Some(TemplateKind::Excel));
        assert_eq!(d.fill_strategy, FillStrategy::ReplaceFieldWithText);
        assert_eq!(d.missing_policy, MissingPolicy::Blank);
        assert_eq!(d.output_scope, OutputScope::BothTablesOneFile);
        assert_eq!(d.manual_blank_fields.len(), 3);
        assert!(d
            .fields
            .iter()
            .any(|f| f.merge_field == "合同约定_付款条件（保持与书面合同一致）"));
        assert!(d.fields.iter().any(|f| f.merge_field == "本次付款数大写"));
    }

    #[test]
    fn 注册表可按键查找() {
        assert_eq!(
            find_template("payment_voucher_v1").unwrap().id,
            PAYMENT_TEMPLATE.id
        );
        assert!(find_template("not_exist").is_none());
    }

    #[test]
    fn 缺值策略按口径留空或拒绝() {
        assert_eq!(apply_policy(None, MissingPolicy::Blank, "x").unwrap(), "");
        assert_eq!(apply_policy(None, MissingPolicy::Dash, "x").unwrap(), "—");
        assert!(apply_policy(None, MissingPolicy::Reject, "x").is_err());
    }

    fn data_with(pairs: &[(&'static str, &str)]) -> PaymentPrintData {
        let mut fields = Vec::new();
        for b in PAYMENT_FIELDS {
            let v = pairs
                .iter()
                .find(|(n, _)| *n == b.merge_field)
                .map(|(_, v)| (*v).to_string())
                .unwrap_or_default();
            fields.push(ResolvedField {
                merge_field: b.merge_field,
                label: b.label,
                value: v,
                source: b.source.label(),
                note: b.note,
            });
        }
        PaymentPrintData {
            payment_no: "FK-1".into(),
            contract_no: "HT-1".into(),
            fields,
            generated_at: now_iso(),
        }
    }

    #[test]
    fn 九域按语义折叠到版面字段() {
        let d = data_with(&[
            ("本次付款数大写", "陆佰圆整"),
            ("本次付款小写", "600.00"),
            ("项目名称", "XX 公路改建工程"),
            ("中标单位", "YY 路桥有限公司"),
            ("合同改成元", "1000.00"),
            ("审计价改成元", "0.00"),
            ("已付款改成元", "0.00"),
            ("本次付款后累计", "600.00"),
            ("合同约定_付款条件（保持与书面合同一致）", "已完成 60%"),
        ]);
        let v = to_voucher(&d);
        assert_eq!(v.this_amount_cn, "陆佰圆整");
        assert_eq!(v.this_amount, "600.00");
        assert_eq!(v.project_name, "XX 公路改建工程");
        assert_eq!(v.contractor, "YY 路桥有限公司");
        assert_eq!(v.contract_amount, "1000.00");
        assert_eq!(v.audit_amount, "0.00");
        assert_eq!(v.paid_before, "0.00");
        assert_eq!(v.cumulative, "600.00");
        assert_eq!(v.terms, "已完成 60%");
        assert_eq!(v.payment_no, "FK-1");
    }

    #[test]
    fn 缺域折叠成空串而不是占位符() {
        let v = to_voucher(&data_with(&[]));
        for s in [&v.project_name, &v.contractor, &v.terms, &v.this_amount] {
            assert!(s.is_empty());
        }
    }

    #[test]
    fn 打印件名称随张数变化() {
        let one = to_voucher(&data_with(&[]));
        assert!(doc_name(std::slice::from_ref(&one)).contains("拨款单 FK-1"));
        assert_eq!(doc_name(&[]), "拨款单");
        let many = vec![one; 3];
        assert!(doc_name(&many).contains("等 3 张"));
        assert_eq!(page_count(many.len()), 6, "每张两页");
    }
}
