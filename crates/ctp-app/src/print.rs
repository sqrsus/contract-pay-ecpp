//! Word 模板的**成品渲染**：内置 docx 模板 → 按域边界替换 9 个合并域 →
//! 单份或多份（批量）合成一个 docx 字节流。
//!
//! 模板实物解析结论（2026-11-05 复核）：11 处域 / 9 个唯一域名、两张表同在一个 docx、
//! 无动态明细行；`w:fldCharType` 三态各 11 次且无嵌套；其中 4 个域的 `«…»` 被 Word
//! 拆成 3 个 run，因此填充必须按**域边界**处理（见 `docx` 模块），不能按字符串替换。
//!
//! 输出通道已确认（R-27）：打开 Word 由用户按打印 → 弹系统打印对话框出纸；
//! 同时保留「导出 Word」与「导出 PDF」（PDF 交给本机 Word 固定版式导出）。

use crate::docx::{self, FieldValue};
use ctp_core::date::{now_iso, today_iso};
use ctp_core::money::Money;
use ctp_core::payment::PaymentStatus;
use ctp_store::{repo, StoreError};
use rusqlite::Connection;
use std::fmt;
use std::path::Path;

/// 内置的 Word 模板（已确认「模板打包进程序」，不依赖外部文件）。
///
/// 文件名用 ASCII：`include_bytes!` 里出现中文路径在不同工具链上容易出岔子。
pub const TEMPLATE_BYTES: &[u8] = include_bytes!("../templates/payment_voucher.docx");

// ---------------------------------------------------------------- 模板决策常量

/// 模板类型。接口保持中立，Word 先行、Excel 留作第二个 adapter。
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
            TemplateKind::Word => "Word 邮件合并域模板",
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

pub static PAYMENT_TEMPLATE: TemplateDescriptor = TemplateDescriptor {
    id: "payment_voucher_v1",
    name: "付款打印模板（财务支出审核监督表 + 工程项目进度款支付申请表）",
    kind: TemplateKind::Word,
    alt_kind: Some(TemplateKind::Excel),
    template_file_hint: "templates/付款打印模板.docx",
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
    #[error("打印模板文件缺失或不可读：{0}")]
    MissingTemplate(String),

    #[error("字段缺失且当前策略为拒绝生成：{0}")]
    MissingField(&'static str),

    #[error("生成打印件失败：{0}")]
    Template(String),

    #[error("该拨款单已作废，不允许打印：{0}")]
    Voided(String),

    #[error("没有选中任何拨款单")]
    NothingSelected,

    #[error("导出 PDF 失败：{0}")]
    Pdf(String),

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
    let paid_before = repo::paid_before_payment(
        conn,
        &pm.contract_id,
        &pm.pay_date,
        &pm.payment_no,
    )?;
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

// ---------------------------------------------------------------- 成品渲染

/// 把解析好的 9 个域转成 docx 填充用的（域名, 取值）对。
///
/// 域名的口径由 `PAYMENT_FIELDS` 固定，`resolve()` 已按同一份描述符产出，两边顺序一致。
pub fn data_values(data: &PaymentPrintData) -> Vec<FieldValue> {
    data.fields
        .iter()
        .map(|f| FieldValue::new(f.merge_field, f.value.clone()))
        .collect()
}

/// 用内置模板生成 docx 字节流。`copies` 的每一份对应一张单据，一份一页。
pub fn render_docx(copies: &[Vec<FieldValue>]) -> Result<Vec<u8>, PrintError> {
    let template = docx::read_entry(TEMPLATE_BYTES, docx::DOC_XML).map_err(|e| {
        PrintError::MissingTemplate(format!("内置打印模板不可读（{e}）；请确认程序完整"))
    })?;
    let merged =
        docx::render_document(&template, copies).map_err(|e| PrintError::Template(e.to_string()))?;
    docx::rewrite_entry(TEMPLATE_BYTES, docx::DOC_XML, &merged)
        .map_err(|e| PrintError::Template(e.to_string()))
}

/// 一次输出的成品：字节流 + 张数 + 建议文件名。
#[derive(Debug)]
pub struct RenderedVoucher {
    pub bytes: Vec<u8>,
    pub count: usize,
    /// 导出弹窗的默认文件名（不含目录）
    pub default_name: String,
    /// 手册核对用：参与输出的单号（按输出顺序）
    pub payment_nos: Vec<String>,
}

impl RenderedVoucher {
    /// PDF 导出时用的默认文件名（同主名换后缀）。
    pub fn pdf_name(&self) -> String {
        self.default_name.replace(".docx", ".pdf")
    }
}

/// 生成一张或一批拨款单的打印件。
///
/// * 作废单一律拒绝（已确认「作废不允许打印」）；
/// * 批量按传入顺序依次成页，每份一份（已确认「勾选若干条 → 合并输出」）。
pub fn render_payments(
    conn: &Connection,
    payment_ids: &[String],
) -> Result<RenderedVoucher, PrintError> {
    if payment_ids.is_empty() {
        return Err(PrintError::NothingSelected);
    }
    let mut copies: Vec<Vec<FieldValue>> = Vec::with_capacity(payment_ids.len());
    let mut nos: Vec<String> = Vec::with_capacity(payment_ids.len());
    for id in payment_ids {
        let pm = repo::get_payment(conn, id)?;
        if pm.status != PaymentStatus::Paid {
            return Err(PrintError::Voided(pm.payment_no));
        }
        let data = resolve(conn, id)?;
        nos.push(data.payment_no.clone());
        copies.push(data_values(&data));
    }
    let bytes = render_docx(&copies)?;
    let default_name = if nos.len() == 1 {
        format!("拨款单_{}.docx", nos[0])
    } else {
        format!(
            "拨款单_{}_等{}张_{}.docx",
            nos[0],
            nos.len(),
            today_iso().replace('-', "")
        )
    };
    Ok(RenderedVoucher {
        bytes,
        count: nos.len(),
        default_name,
        payment_nos: nos,
    })
}

// ---------------------------------------------------------------- 适配器

/// 渲染入参：业务层 / 界面都不接触模板文件格式细节。
pub struct PrintContext<'a> {
    pub descriptor: &'static TemplateDescriptor,
    pub data: &'a PaymentPrintData,
    pub template_path: &'a Path,
    pub output_path: &'a Path,
}

/// 打印模板填充器。Word 已实现；Excel 留作第二个 adapter。
pub trait PrintAdapter {
    fn kind(&self) -> TemplateKind;
    fn render(&self, ctx: &PrintContext<'_>) -> Result<(), PrintError>;
}

/// Word 模板填充器：内置模板 → 域替换 → 落盘。
///
/// `template_path` 不再使用（模板已打包进程序），保留字段只为接口稳定。
pub struct WordAdapter;

impl PrintAdapter for WordAdapter {
    fn kind(&self) -> TemplateKind {
        TemplateKind::Word
    }

    fn render(&self, ctx: &PrintContext<'_>) -> Result<(), PrintError> {
        let bytes = render_docx(&[data_values(ctx.data)])?;
        std::fs::write(ctx.output_path, bytes)
            .map_err(|e| PrintError::Template(format!("写入打印件失败：{e}")))
    }
}

pub fn adapters() -> Vec<Box<dyn PrintAdapter>> {
    vec![Box::new(WordAdapter)]
}

/// 统一的落盘入口：把一张单据写成 docx 文件。
pub fn render(ctx: &PrintContext<'_>) -> Result<(), PrintError> {
    let kind = ctx.descriptor.kind;
    match adapters().into_iter().find(|a| a.kind() == kind) {
        Some(adapter) => adapter.render(ctx),
        None => Err(PrintError::Template(format!(
            "{} 的填充实现尚未接入",
            kind.label()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 描述符固化了九域与已确认口径() {
        let d = &PAYMENT_TEMPLATE;
        assert_eq!(d.fields.len(), 9);
        assert_eq!(d.kind, TemplateKind::Word);
        assert_eq!(d.alt_kind, Some(TemplateKind::Excel));
        assert_eq!(d.fill_strategy, FillStrategy::ReplaceFieldWithText);
        assert_eq!(d.missing_policy, MissingPolicy::Blank);
        assert_eq!(d.output_scope, OutputScope::BothTablesOneFile);
        assert_eq!(d.manual_blank_fields.len(), 3);
        assert!(d.fields.iter().any(|f| f.merge_field == "合同约定_付款条件（保持与书面合同一致）"));
        assert!(d.fields.iter().any(|f| f.merge_field == "本次付款数大写"));
    }

    #[test]
    fn 注册表与填充实现已接入() {
        assert_eq!(find_template("payment_voucher_v1").unwrap().id, PAYMENT_TEMPLATE.id);
        assert!(find_template("not_exist").is_none());
        assert_eq!(adapters().len(), 1, "Word 填充器已接入");
        assert_eq!(adapters()[0].kind(), TemplateKind::Word);
    }

    #[test]
    fn 内置模板可读且九域都在() {
        let xml = docx::read_entry(TEMPLATE_BYTES, docx::DOC_XML).expect("内置模板应可读");
        let names = docx::field_names(&xml);
        assert_eq!(names.len(), 11, "模板内 11 处合并域");
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 9, "9 个唯一域名");
        // 描述符覆盖了模板里的所有域名（含带 \# "0.00" 开关的四个）
        for n in &unique {
            assert!(
                PAYMENT_FIELDS.iter().any(|f| f.merge_field == n),
                "描述符缺少域名 {n}"
            );
        }
    }

    #[test]
    fn 单张与批量都能生成且无残留占位符() {
        let values: Vec<FieldValue> = PAYMENT_FIELDS
            .iter()
            .enumerate()
            .map(|(i, f)| FieldValue::new(f.merge_field, format!("v{i}")))
            .collect();
        let one = render_docx(&[values.clone()]).expect("单张生成失败");
        assert!(!one.is_empty());
        let two = render_docx(&[values.clone(), values]).expect("批量生成失败");
        assert!(two.len() > one.len(), "两份应比一份大");
        for bytes in [&one, &two] {
            let xml = docx::read_entry(bytes, docx::DOC_XML).expect("成品应可读");
            assert!(docx::residual_fields(&xml).is_empty(), "成品不应残留合并域");
            assert!(!xml.contains("MERGEFIELD"), "成品不应残留域代码");
        }
    }

    #[test]
    fn 缺值策略按口径留空或拒绝() {
        assert_eq!(
            apply_policy(None, MissingPolicy::Blank, "x").unwrap(),
            ""
        );
        assert_eq!(
            apply_policy(None, MissingPolicy::Dash, "x").unwrap(),
            "—"
        );
        assert!(apply_policy(None, MissingPolicy::Reject, "x").is_err());
    }
}
