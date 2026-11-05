//! 集成测试：把评审意见中的每一条口径结论，写成可执行断言。
//!
//! | 用例 | 对应评审条目 |
//! |---|---|
//! | `比率口径_视图与应用层差异为零` | H1 / 5.5 约束第 5 条 |
//! | `已付口径_作废单不计入净额` | H2 / H5 / R-20 |
//! | `超付拦截_拒绝并给出差额` | BR-01 / E-02 |
//! | `容差内放行_超容差拦截` | BR-15 三支处置 |
//! | `审计价切换_已付不变基准回溯` | BR-14 / E-00 |
//! | `金额编辑留痕_已审定不改基准` | BR-22 |
//! | `草稿合同不可拨款` | 4.2.2 is_payable |
//! | `会计分类与合同类别分离` | H4 |
//! | `业务表审计列齐全` | M4 / DB-06 |

use ctp_app::service::{self, NewContract, NewPayment};
use ctp_core::attachment::AttachmentKind;
use ctp_core::contract::ContractStatus;
use ctp_core::error::DomainError;
use ctp_core::money::Money;
use ctp_core::project::{Contractor, ContractorRole, Project};
use ctp_core::quota::BreachLevel;
use ctp_core::rate::Rate;
use ctp_store::{db, repo};
use rusqlite::Connection;

fn project_and_org(conn: &Connection) {
    let now = ctp_core::date::now_iso();
    repo::insert_project(
        conn,
        &Project {
            id: "PJ-1".into(),
            project_code: "XAJT".into(),
            project_name: "测试项目".into(),
            remark: None,
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some("t".into()),
            updated_at: now.clone(),
            updated_by: Some("t".into()),
        },
    )
    .unwrap();
    repo::insert_contractor(
        conn,
        &Contractor {
            id: "ORG-1".into(),
            contractor_name: "某施工单位".into(),
            role_type: ContractorRole::Construct,
            credit_code: None,
            bank_name: None,
            bank_account: None,
            remark: None,
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some("t".into()),
            updated_at: now,
            updated_by: Some("t".into()),
        },
    )
    .unwrap();
}

fn fixture(amount_yuan: i64) -> (Connection, String) {
    // 默认「非施工 + 已交工」：不受付清条件封顶，用于验证原始额度口径。
    fixture_with(
        amount_yuan,
        ctp_core::contract::ContractType::Service,
        Some("2020-01-01"),
    )
}

fn fixture_with(
    amount_yuan: i64,
    ctype: ctp_core::contract::ContractType,
    delivery: Option<&str>,
) -> (Connection, String) {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);
    let c = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "测试合同".into(),
            contract_type: ctype,
            contract_category_code: "GC".into(),
            category_id: Some("C02".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(amount_yuan * 100),
            delivery_date: delivery.map(str::to_string),
            payment_terms: "按月计量支付".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();
    attach_contract(&conn, &c.id);
    service::activate_contract(&mut conn, &c.id, "t").unwrap();
    (conn, c.id)
}

/// 补一份「合同正本」附件：生效校验要求它必须在位。
fn attach_contract(conn: &Connection, contract_id: &str) {
    service::save_attachment(
        conn,
        contract_id,
        ctp_core::attachment::AttachmentKind::Contract,
        "合同正本.pdf",
        Some("application/pdf"),
        b"%PDF-1.4 test",
        "t",
    )
    .unwrap();
}

fn pay(conn: &mut Connection, contract_id: &str, yuan: i64) -> ctp_store::Result<ctp_core::payment::Payment> {
    service::register_payment(conn, NewPayment::new(contract_id, Money::from_cent(yuan * 100), "2026-03-31"), "t")
}

#[test]
fn 比率口径_视图与应用层差异为零() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_app::demo::seed_demo(&mut conn, "t").unwrap();

    assert!(service::reconcile(&conn).unwrap().is_empty(), "H1：视图与 Rate::ratio 必须逐位一致");

    // 逐条复核：视图比率 = 已付 / 有效基准
    for s in repo::summaries(&conn).unwrap() {
        let expected = Rate::ratio(s.paid_amount, s.base_amount);
        assert_eq!(s.pay_rate.bp(), expected.bp(), "合同 {}", s.contract_no);
    }
}

#[test]
fn 已付口径_作废单不计入净额() {
    let (mut conn, cid) = fixture(1000);
    let p = pay(&mut conn, &cid, 300).unwrap();
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 30_000);

    service::void_payment(&mut conn, &p.id, "录错，作废重登", "t").unwrap();
    assert_eq!(
        repo::paid_net(&conn, &cid).unwrap().cent(),
        0,
        "H5：作废单不计入已付净额"
    );
    let s = repo::summary(&conn, &cid).unwrap().unwrap();
    assert_eq!(s.paid_amount.cent(), 0, "V1 与仓储必须同口径");
    assert_eq!(s.pay_rate.bp(), 0);

    // 重登后恢复
    pay(&mut conn, &cid, 200).unwrap();
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 20_000);
}

#[test]
fn 超付拦截_拒绝并给出差额() {
    let (mut conn, cid) = fixture(1000);
    pay(&mut conn, &cid, 600).unwrap();

    let err = pay(&mut conn, &cid, 500).unwrap_err();
    match err.domain() {
        Some(DomainError::QuotaExceeded {
            requested,
            available,
            over,
        }) => {
            assert_eq!(*requested, 50_000);
            assert_eq!(*available, 40_000);
            assert_eq!(*over, 10_000);
        }
        other => panic!("应为额度过错，实得 {other:?}"),
    }

    // 拦截不得写入任何流水
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 60_000);
    assert_eq!(repo::paid_payment_count(&conn, &cid).unwrap(), 1);

    // 边界：恰好用满可以通过
    pay(&mut conn, &cid, 400).unwrap();
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 100_000);
    let q = service::quota_of(&conn, &cid).unwrap();
    assert!(q.is_settled());
    assert_eq!(q.pay_rate.bp(), 10_000);
}

#[test]
fn 容差内放行_超容差拦截() {
    let (mut conn, cid) = fixture(1000);
    service::set_tolerance(&mut conn, Rate::from_bp(100), "t").unwrap(); // 1%

    pay(&mut conn, &cid, 1005).unwrap(); // 超基准但在容差内
    let q = service::quota_of(&conn, &cid).unwrap();
    assert_eq!(q.ceiling_amount.cent(), 101_000);
    assert_eq!(q.available_amount.cent(), 500);
    assert!(q.is_inverted, "超基准即标红，标记不含容差");
    assert_eq!(q.breach_level(), BreachLevel::WithinTolerance);

    // 再拨 6 元即超容差
    let err = pay(&mut conn, &cid, 6).unwrap_err();
    assert!(err.is_quota_exceeded());
    // 容差内剩余 5 元可拨
    pay(&mut conn, &cid, 5).unwrap();
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 101_000);
}

#[test]
fn 未交工合同封顶八成() {
    let (mut conn, cid) = fixture_with(1000, ctp_core::contract::ContractType::Construct, None);

    pay(&mut conn, &cid, 700).unwrap();
    let err = pay(&mut conn, &cid, 101).unwrap_err();
    assert!(err.is_payment_blocked(), "应为付清条件拦截，实得 {err}");

    // 恰好推到有效计价基准的 80% 可以通过
    pay(&mut conn, &cid, 100).unwrap();
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 80_000);
    // 再付 1 分即被拦
    assert!(pay(&mut conn, &cid, 1).unwrap_err().is_payment_blocked());

    let q = service::quota_on(&conn, &cid, "2026-03-31").unwrap();
    assert_eq!(q.cap_bp, 8_000);
    assert_eq!(q.payable_amount.cent(), 0);
    assert!(q.limit_note.unwrap().contains("未登记交工日期"));
}

#[test]
fn 施工合同付清需审定且交工满两年() {
    // 交工 2024-06-01：距开单日 2026-03-31 不满 2 年
    let (mut conn, cid) = fixture_with(
        1000,
        ctp_core::contract::ContractType::Construct,
        Some("2024-06-01"),
    );

    // 未审定 → 80% 封顶
    assert!(pay(&mut conn, &cid, 900).unwrap_err().is_payment_blocked());
    pay(&mut conn, &cid, 800).unwrap();

    // 补登记审计价，仍不满 2 年 → 不得付清
    service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(100_000)),
        None,
        None,
        None,
        "结算审定",
        "t",
    )
    .unwrap();
    let err = pay(&mut conn, &cid, 200).unwrap_err();
    assert!(err.is_payment_blocked(), "交工未满 2 年不得付清，实得 {err}");

    // 交工日期提前到 2023-01-01（+2 年 = 2025-01-01 < 开单日）→ 可付清
    service::set_delivery_date(&mut conn, &cid, "2023-01-01", "t").unwrap();
    pay(&mut conn, &cid, 200).unwrap();
    let q = service::quota_on(&conn, &cid, "2026-03-31").unwrap();
    assert!(q.is_settled());
    assert_eq!(q.cap_bp, 10_000);
    assert!(q.limit_note.is_none());
}

#[test]
fn 满两年当日仍不可付清() {
    // 交工 2024-03-31，开单 2026-03-31：同日不算「大于 2 年」
    let (mut conn, cid) = fixture_with(
        1000,
        ctp_core::contract::ContractType::Construct,
        Some("2024-03-31"),
    );
    service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(100_000)),
        None,
        None,
        None,
        "结算审定",
        "t",
    )
    .unwrap();
    pay(&mut conn, &cid, 800).unwrap();
    assert!(pay(&mut conn, &cid, 200).unwrap_err().is_payment_blocked());

    let q = service::quota_on(&conn, &cid, "2026-04-01").unwrap();
    assert_eq!(q.cap_bp, 10_000, "开单日推到满 2 年之后即可付清");
}

#[test]
fn 审计价切换_已付不变基准回溯() {
    let (mut conn, cid) = fixture(1000);
    pay(&mut conn, &cid, 800).unwrap();

    let c = service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(90_000)),
        Some("2026-10-20".into()),
        Some("某造价咨询".into()),
        Some("SJ-001".into()),
        "竣工结算审定",
        "t",
    )
    .unwrap();
    assert_eq!(c.total_amount().cent(), 100_000, "合同金额只作档案");
    assert_eq!(c.effective_base().cent(), 90_000);
    assert_eq!(c.audit_reduction().cent(), 10_000);

    // 已付不变，比率与额度按新基准重算
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 80_000);
    let s = repo::summary(&conn, &cid).unwrap().unwrap();
    assert_eq!(s.base_amount.cent(), 90_000);
    assert_eq!(s.paid_amount.cent(), 80_000);
    assert_eq!(s.pay_rate.bp(), 8_889); // 800/900 = 88.888…% → 8889 bp（四舍五入）
    assert_eq!(s.available_amount.cent(), 10_000);
    assert!(!s.is_inverted);

    // 清空审计价 → 基准回退合同金额
    let c = service::register_audit_price(&mut conn, &cid, None, None, None, None, "审定撤回", "t")
        .unwrap();
    assert!(!c.is_audited());
    assert_eq!(c.effective_base().cent(), 100_000);
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 80_000);
}

#[test]
fn 金额编辑留痕_已审定不改基准() {
    let (mut conn, cid) = fixture(1000);
    service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(90_000)),
        None,
        None,
        None,
        "登记审计价",
        "t",
    )
    .unwrap();

    // 必填原因
    let err = service::edit_contract_amount(&mut conn, &cid, Money::from_cent(110_000), "  ", "t")
        .unwrap_err();
    assert!(matches!(err.domain(), Some(DomainError::MissingField("金额编辑原因"))));

    let c = service::edit_contract_amount(
        &mut conn,
        &cid,
        Money::from_cent(110_000),
        "设计变更增加工程量",
        "t",
    )
    .unwrap();
    assert_eq!(c.total_amount().cent(), 110_000);
    assert_eq!(c.effective_base().cent(), 90_000, "BR-14：已审定不改变基准");
    assert_eq!(c.audit_reduction().cent(), 20_000);

    let hist = ctp_app::history::load(&conn, ctp_app::history::KEY_CONTRACT_AMOUNT).unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0]["old_cent"], 100_000);
    assert_eq!(hist[0]["new_cent"], 110_000);
    assert_eq!(hist[0]["reason"], "设计变更增加工程量");
}

#[test]
fn 草稿合同不可拨款() {
    let (mut conn, cid) = fixture(1000);
    // 退回草稿：直接改状态（本期无「撤回」动作）
    let mut c = repo::get_contract(&conn, &cid).unwrap();
    c.status = ContractStatus::Draft;
    repo::update_contract(&conn, &c).unwrap();

    let err = pay(&mut conn, &cid, 100).unwrap_err();
    assert!(matches!(
        err.domain(),
        Some(DomainError::ContractNotPayable(_))
    ));
}

#[test]
fn 新建合同先草稿再生效后才能拨款() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);
    let c = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "新增合同".into(),
            contract_category_code: "GC".into(),
            category_id: Some("C02".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-02-10".into(),
            original_amount: Money::from_cent(500_000),
            payment_terms: "按月计量支付".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();

    // 1) 新建即草稿，草稿拒付
    assert_eq!(c.status, ContractStatus::Draft);
    let err = pay(&mut conn, &c.id, 100).unwrap_err();
    assert!(matches!(
        err.domain(),
        Some(DomainError::ContractNotPayable(_))
    ));

    // 2) 没有合同附件时不允许生效（合同正本为必传项）
    let err = service::activate_contract(&mut conn, &c.id, "t").unwrap_err();
    assert!(matches!(
        err.domain(),
        Some(DomainError::AttachmentRequired(_))
    ));

    // 3) 补齐合同附件后可生效
    attach_contract(&conn, &c.id);
    let a = service::activate_contract(&mut conn, &c.id, "t").unwrap();
    assert_eq!(a.status, ContractStatus::Active);
    assert!(a.is_payable());

    // 4) 首笔拨款把「生效」推进到「履行中」
    pay(&mut conn, &c.id, 100).unwrap();
    let after = repo::get_contract(&conn, &c.id).unwrap();
    assert_eq!(after.status, ContractStatus::Performing);

    // 5) 已是履行中，重复生效被状态机拒绝
    assert!(service::activate_contract(&mut conn, &c.id, "t").is_err());
}

#[test]
fn 会计分类与合同类别分离() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_app::demo::seed_demo(&mut conn, "t").unwrap();

    let rows = repo::category_summary(&conn).unwrap();
    assert!(!rows.is_empty());
    for r in &rows {
        assert!(
            ctp_core::dict::is_valid_accounting_category(&r.category_code),
            "汇总必须按会计分类，实得 {} {}",
            r.category_code,
            r.category_name
        );
    }
    // 合同类别不得出现在会计分类表里
    let names: Vec<&str> = rows.iter().map(|r| r.category_name.as_str()).collect();
    for biz in ctp_core::dict::CONTRACT_CATEGORIES {
        assert!(!names.contains(&biz.1), "合同类别 {} 混入了会计分类汇总", biz.1);
    }
    // 分类合计 = 各合同有效基准之和
    let total_cat: i64 = rows.iter().map(|r| r.base_amount.cent()).sum();
    let total_contract: i64 = repo::summaries(&conn)
        .unwrap()
        .iter()
        .map(|s| s.base_amount.cent())
        .sum();
    assert_eq!(total_cat, total_contract, "分类合计必须闭合");
}

#[test]
fn 业务表审计列齐全() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    let bad = repo::audit_column_violations(&conn).unwrap();
    assert!(bad.is_empty(), "M4：{bad:?}");
}

/// 每轮导出用独立临时文件，避免并行测试互相覆盖。
fn tmp_xlsx(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ctp_{}_{}_{}.xlsx",
        tag,
        std::process::id(),
        ctp_core::date::now_iso().replace([':', '-', 'T', 'Z'], "")
    ))
}

#[test]
fn 报表导出与屏幕共用结果且写审计日志() {
    let (mut conn, cid) = fixture(1_000_000);
    pay(&mut conn, &cid, 100_000).unwrap();

    // 屏幕与导出读同一份 ReportResult 构造逻辑，金额口径取自 V3 / V4 视图
    let cat = service::report_of(&conn, ctp_app::report::ReportKind::CategorySummary).unwrap();
    assert!(!cat.is_empty());
    let row = cat
        .rows
        .iter()
        .find(|r| r[0].display() == "C02")
        .expect("C02 分类应在 V3 里");
    assert_eq!(row[3].yuan(), Some(1_000_000.0));

    let path = tmp_xlsx("period");
    let (rows, cols) = service::export_report(
        &conn,
        ctp_app::report::ReportKind::PeriodSummary,
        &path,
        "t",
    )
    .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(cols, 4);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[0..2], b"PK", "xlsx 必须是 zip 容器");
    let _ = std::fs::remove_file(&path);

    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sys_audit_log WHERE action = 'EXPORT' \
             AND target_id = 'PERIOD_SUMMARY_V4'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "FR-SYS-07：每次导出应留一条 EXPORT 日志");
}

#[test]
fn 付款打印九域解析与中文大写() {
    let (mut conn, cid) = fixture(1_000_000);
    pay(&mut conn, &cid, 100_000).unwrap();
    let pm = pay(&mut conn, &cid, 250_000).unwrap();

    let data = service::payment_print_data(&conn, &pm.id).unwrap();
    assert_eq!(data.fields.len(), 9);
    assert_eq!(data.get("项目名称"), Some("测试合同"), "取合同名称，非项目名称");
    assert_eq!(data.get("中标单位"), Some("某施工单位"), "取承包单位，非发包人");
    assert_eq!(data.get("合同改成元"), Some("1000000.00"));
    assert_eq!(data.get("审计价改成元"), Some("0.00"), "未审定填 0");
    assert_eq!(data.get("已付款改成元"), Some("100000.00"), "本次之前的累计已付");
    assert_eq!(data.get("本次付款小写"), Some("250000.00"));
    assert_eq!(data.get("本次付款数大写"), Some("贰拾伍万圆整"));
    assert_eq!(data.get("本次付款后累计"), Some("350000.00"));
    assert_eq!(
        data.get("合同约定_付款条件（保持与书面合同一致）"),
        Some("按月计量支付")
    );
}

#[test]
fn 打印输出本期明确未实现且接口中立() {
    use ctp_app::print::{PaymentPrintData, PrintContext, PrintError, TemplateKind, PAYMENT_TEMPLATE};

    assert!(ctp_app::print::adapters().is_empty(), "本轮不接入填充实现");
    assert_eq!(PAYMENT_TEMPLATE.kind, TemplateKind::Word);
    assert_eq!(PAYMENT_TEMPLATE.alt_kind, Some(TemplateKind::Excel));

    let data = PaymentPrintData {
        payment_no: "X".into(),
        contract_no: "Y".into(),
        fields: Vec::new(),
        generated_at: ctp_core::date::now_iso(),
    };
    let ctx = PrintContext {
        descriptor: &PAYMENT_TEMPLATE,
        data: &data,
        template_path: std::path::Path::new("templates/付款打印模板.docx"),
        output_path: std::path::Path::new("out.docx"),
    };
    let err = ctp_app::print::render(&ctx).unwrap_err();
    assert!(matches!(err, PrintError::NotImplemented(TemplateKind::Word)));
}

#[test]
fn 合同编号按年度递增且唯一() {
    let (mut conn, _cid) = fixture(1000);
    assert_eq!(
        repo::find_contract_by_no(&conn, "HT-2026-001").unwrap().is_some(),
        true
    );
    let c2 = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "第二份合同".into(),
            contract_category_code: "GC".into(),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-02-01".into(),
            original_amount: Money::from_cent(100_000),
            payment_terms: "月结".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();
    assert_eq!(c2.contract_no, "HT-2026-002");
}

#[test]
fn 合同必填项与字典校验生效() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);

    // 缺付款条款
    let err = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "无付款条款".into(),
            contract_category_code: "GC".into(),
            party_a: "甲方".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(1000),
            payment_terms: "".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap_err();
    assert!(matches!(err.domain(), Some(DomainError::MissingField("付款条款"))));

    // 合同类别填成会计分类（H4 的典型误用）
    let err = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "类别串号".into(),
            contract_category_code: "C01".into(),
            party_a: "甲方".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(1000),
            payment_terms: "月结".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap_err();
    assert!(matches!(err.domain(), Some(DomainError::InvalidDict(_))));
}

// ---------------------------------------------------------------- 附件与自动建档

#[test]
fn 名称输入自动建档_同名复用不重复() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    // 刻意不插任何项目与单位：完全靠输入框文本建档

    let mk = |name: &str, date: &str| NewContract {
        project_text: Some("新建市政道路工程".into()),
        contractor_text: Some("某某建工集团有限公司".into()),
        party_a: "某某市城建局".into(),
        contract_name: name.into(),
        contract_category_code: "GC".into(),
        sign_date: date.into(),
        original_amount: Money::from_cent(100_000),
        payment_terms: "月结".into(),
        ..Default::default()
    };

    let c = service::create_contract(&mut conn, mk("自动建档一", "2026-03-01"), "t").unwrap();
    assert_eq!(c.project_id.len() > 0, true);

    let projects = repo::list_projects(&conn).unwrap();
    assert_eq!(projects.len(), 1);
    assert!(projects[0].project_code.starts_with("PRJ-"));
    assert_eq!(projects[0].project_name, "新建市政道路工程");

    let orgs = repo::list_contractors(&conn).unwrap();
    assert_eq!(orgs.len(), 2, "承包单位 + 甲方各一条");
    let owner = orgs
        .iter()
        .find(|x| x.contractor_name == "某某市城建局")
        .unwrap();
    assert_eq!(owner.role_type, ContractorRole::Owner);
    let builder = orgs
        .iter()
        .find(|x| x.contractor_name == "某某建工集团有限公司")
        .unwrap();
    assert_eq!(builder.role_type, ContractorRole::Construct);

    // 第二份合同用同样三个名称：全部复用，不新增档案
    service::create_contract(&mut conn, mk("自动建档二", "2026-04-01"), "t").unwrap();
    assert_eq!(repo::list_projects(&conn).unwrap().len(), 1);
    assert_eq!(repo::list_contractors(&conn).unwrap().len(), 2);
}

#[test]
fn 附件每类一份_替换即覆盖且可删除() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);
    let c = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "附件测试".into(),
            contract_category_code: "GC".into(),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(100_000),
            payment_terms: "月结".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();

    // 没附件不能生效
    assert!(service::activate_contract(&mut conn, &c.id, "t").is_err());

    service::save_attachment(
        &conn,
        &c.id,
        AttachmentKind::Contract,
        "合同正本.pdf",
        Some("application/pdf"),
        b"PDF-1",
        "t",
    )
    .unwrap();
    service::save_attachment(
        &conn,
        &c.id,
        AttachmentKind::BidNotice,
        "中标通知书.pdf",
        None,
        b"BID",
        "t",
    )
    .unwrap();
    assert_eq!(service::list_attachments(&conn, &c.id).unwrap().len(), 2);

    // 同类再传：覆盖而不是新增
    service::save_attachment(
        &conn,
        &c.id,
        AttachmentKind::Contract,
        "合同正本-v2.pdf",
        None,
        b"PDF-2-longer",
        "t",
    )
    .unwrap();
    let list = service::list_attachments(&conn, &c.id).unwrap();
    assert_eq!(list.len(), 2, "同类仍是两条中的一条");
    let con = list.iter().find(|a| a.kind_code == "CONTRACT").unwrap();
    assert_eq!(con.file_name, "合同正本-v2.pdf");
    assert_eq!(con.size_bytes, 12);
    assert_eq!(service::attachment_bytes(&conn, &con.id).unwrap(), b"PDF-2-longer");

    // 合同正本在位后可生效
    service::activate_contract(&mut conn, &c.id, "t").unwrap();

    // 删除后计数下降
    service::delete_attachment(&conn, &con.id, "t").unwrap();
    assert_eq!(service::list_attachments(&conn, &c.id).unwrap().len(), 1);
}

#[test]
fn 附件空文件与超限被拒() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);
    let c = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "附件边界".into(),
            contract_category_code: "GC".into(),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(100_000),
            payment_terms: "月结".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();

    let empty = service::save_attachment(
        &conn,
        &c.id,
        AttachmentKind::Contract,
        "空.pdf",
        None,
        b"",
        "t",
    )
    .unwrap_err();
    assert!(empty.to_string().contains("为空"));

    let over = vec![0u8; (ctp_core::attachment::MAX_ATTACHMENT_BYTES + 1) as usize];
    let err = service::save_attachment(
        &conn,
        &c.id,
        AttachmentKind::Contract,
        "超大.pdf",
        None,
        &over,
        "t",
    )
    .unwrap_err();
    assert!(err.to_string().contains("上限"));
}

