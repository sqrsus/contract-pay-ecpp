//! 集成测试：把评审意见中的每一条口径结论，写成可执行断言。
//!
//! | 用例 | 对应评审条目 |
//! |---|---|
//! | `比率口径_视图与应用层差异为零` | H1 / 5.5 约束第 5 条 |
//! | `已付口径_作废单不计入净额` | H2 / H5 / R-20 |
//! | `超付拦截_拒绝并给出差额` | BR-01 / E-02 |
//! | `容差固定为零_超基准即拦截` | BR-15 / R-33 |
//! | `审计价切换_已付不变基准回溯` | BR-14 / E-00 |
//! | `金额编辑留痕_已审定不改基准` | BR-22 |
//! | `草稿合同不可拨款` | 4.2.2 is_payable |
//! | `会计分类与合同类别分离` | H4 |
//! | `业务表审计列齐全` | M4 / DB-06 |

use ctp_app::service::{self, EditContract, NewContract, NewPayment};
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
    attach_contract(&mut conn, &c.id);
    service::activate_contract(&mut conn, &c.id, "t").unwrap();
    (conn, c.id)
}

/// 补一份「合同正本」附件：生效校验要求它必须在位。
fn attach_contract(conn: &mut Connection, contract_id: &str) {
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

fn pay(
    conn: &mut Connection,
    contract_id: &str,
    yuan: i64,
) -> ctp_store::Result<ctp_core::payment::Payment> {
    service::register_payment(
        conn,
        NewPayment::new(contract_id, Money::from_cent(yuan * 100), "2026-03-31"),
        "t",
    )
}

#[test]
fn 比率口径_视图与应用层差异为零() {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_app::demo::seed_demo(&mut conn, "t").unwrap();

    assert!(
        service::reconcile(&conn).unwrap().is_empty(),
        "H1：视图与 Rate::ratio 必须逐位一致"
    );

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
fn 容差固定为零_超基准即拦截() {
    let (mut conn, cid) = fixture(1000);
    // R-33：界面与命令行都不再能改容差；旧库里残留的非 0 值也必须被忽略
    db::set_meta(&conn, "tol_bp", "100", "t").unwrap();

    let q = service::quota_of(&conn, &cid).unwrap();
    assert_eq!(q.tolerance.bp(), 0);
    assert_eq!(q.ceiling_amount.cent(), 100_000, "上限即有效计价基准");

    pay(&mut conn, &cid, 1000).unwrap(); // 正好付清
    let q = service::quota_of(&conn, &cid).unwrap();
    assert_eq!(q.available_amount.cent(), 0);
    assert!(!q.is_inverted, "已付等于基准不算倒挂");
    assert_eq!(q.breach_level(), BreachLevel::None);

    let err = pay(&mut conn, &cid, 1).unwrap_err();
    assert!(err.is_quota_exceeded(), "固定 0 下不允许再超基准");
    assert_eq!(repo::paid_net(&conn, &cid).unwrap().cent(), 100_000);
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
    assert!(
        err.is_payment_blocked(),
        "交工未满 2 年不得付清，实得 {err}"
    );

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
    assert!(matches!(
        err.domain(),
        Some(DomainError::MissingField("金额编辑原因"))
    ));

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
    attach_contract(&mut conn, &c.id);
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
        assert!(
            !names.contains(&biz.1),
            "合同类别 {} 混入了会计分类汇总",
            biz.1
        );
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
    assert_eq!(
        data.get("项目名称"),
        Some("测试合同"),
        "取合同名称，非项目名称"
    );
    assert_eq!(
        data.get("中标单位"),
        Some("某施工单位"),
        "取承包单位，非发包人"
    );
    assert_eq!(data.get("合同改成元"), Some("1000000.00"));
    assert_eq!(data.get("审计价改成元"), Some("0.00"), "未审定填 0");
    assert_eq!(
        data.get("已付款改成元"),
        Some("100000.00"),
        "本次之前的累计已付"
    );
    assert_eq!(data.get("本次付款小写"), Some("250000.00"));
    assert_eq!(data.get("本次付款数大写"), Some("贰拾伍万圆整"));
    assert_eq!(data.get("本次付款后累计"), Some("350000.00"));
    assert_eq!(
        data.get("合同约定_付款条件（保持与书面合同一致）"),
        Some("按月计量支付")
    );
}

#[test]
fn 打印走版面数据且不依赖外部软件() {
    use ctp_app::print::{page_count, TemplateKind, PAYMENT_TEMPLATE};

    // 描述符仍固化 Word 模板来源（版式基准），但输出已改由 paper + GDI 直接排版
    assert_eq!(PAYMENT_TEMPLATE.kind, TemplateKind::Word);
    assert_eq!(PAYMENT_TEMPLATE.alt_kind, Some(TemplateKind::Excel));

    let (mut conn, cid) = fixture(1000);
    let pm = pay(&mut conn, &cid, 250).unwrap();

    // 9 域解析仍是唯一取数入口
    let data = service::payment_print_data(&conn, &pm.id).unwrap();
    assert_eq!(data.get("本次付款小写"), Some("250.00"));

    // 打印入口直接吐版面数据，一张两页
    let vs = service::print_vouchers(&conn, std::slice::from_ref(&pm.id)).unwrap();
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].this_amount, "250.00");
    assert_eq!(page_count(vs.len()), 2);
    assert_eq!(vs[0].payment_no, pm.payment_no);
}

#[test]
fn 合同编号按年度递增且唯一() {
    let (mut conn, _cid) = fixture(1000);
    assert!(repo::find_contract_by_no(&conn, "HT-2026-001")
        .unwrap()
        .is_some());
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
    assert!(matches!(
        err.domain(),
        Some(DomainError::MissingField("付款条款"))
    ));

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
    assert!(!c.project_id.is_empty());

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
        &mut conn,
        &c.id,
        AttachmentKind::Contract,
        "合同正本.pdf",
        Some("application/pdf"),
        b"PDF-1",
        "t",
    )
    .unwrap();
    service::save_attachment(
        &mut conn,
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
        &mut conn,
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
    assert_eq!(
        service::attachment_bytes(&conn, &con.id).unwrap(),
        b"PDF-2-longer"
    );

    // 合同正本在位后可生效
    service::activate_contract(&mut conn, &c.id, "t").unwrap();

    // 删除后计数下降
    service::delete_attachment(&mut conn, &con.id, "t").unwrap();
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
        &mut conn,
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
        &mut conn,
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

/// R-32：四个汇总维度共用同一段过滤，切换维度后合计不变；
/// 项目状态 / 合同类型 / 关键词 / 日期区间四个筛选按口径生效。
#[test]
fn 汇总统计_四维度合计一致_筛选生效() {
    use ctp_core::ledger::{SummaryDim, SummaryFilter};

    // 合同一：服务类 100 万，付 40 万 → 履行中
    let (mut conn, c1) = fixture(1_000_000);
    pay(&mut conn, &c1, 400_000).unwrap();

    // 合同二：服务类 20 万，付清 → 归档关闭
    let c2 = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "测试合同二".into(),
            contract_type: ctp_core::contract::ContractType::Service,
            contract_category_code: "GC".into(),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-15".into(),
            original_amount: Money::from_cent(200_000 * 100),
            delivery_date: Some("2020-01-01".into()),
            payment_terms: "按月计量支付".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();
    attach_contract(&mut conn, &c2.id);
    service::activate_contract(&mut conn, &c2.id, "t").unwrap();
    pay(&mut conn, &c2.id, 200_000).unwrap();

    let all = SummaryFilter::default();
    for dim in SummaryDim::all() {
        let rows = repo::payment_summary(&conn, dim, &all).unwrap();
        let amt: i64 = rows.iter().map(|r| r.amount.cent()).sum();
        assert_eq!(amt, 600_000 * 100, "{dim:?} 全量合计应为 60 万");
        let n: i64 = rows.iter().map(|r| r.payment_count).sum();
        assert_eq!(n, 2, "{dim:?} 全量笔数应为 2");
    }

    // 按合同维度带承包单位列；报表层为 5 列
    let rows = repo::payment_summary(&conn, SummaryDim::Contract, &all).unwrap();
    assert!(
        rows.iter().all(|r| r.sub.as_deref() == Some("某施工单位")),
        "按合同维度应带出承包单位"
    );
    let r5 = service::summary_of(&conn, SummaryDim::Contract, &all).unwrap();
    assert_eq!(r5.columns.len(), 5);

    // 按合同类型的行名翻成中文（SERVICE → 服务）
    let rt = service::summary_of(&conn, SummaryDim::ContractType, &all).unwrap();
    assert!(rt.rows.iter().all(|c| matches!(
        &c[0],
        ctp_app::report::Cell::Text(t) if t == "服务"
    )));

    let sum_cent = |f: &SummaryFilter| -> i64 {
        let rows = repo::payment_summary(&conn, SummaryDim::Contract, f).unwrap();
        rows.iter().map(|r| r.amount.cent()).sum()
    };

    // 状态筛选：归档口径 = 支付率 ≥ 100%（全周期，不受区间影响）
    assert_eq!(
        sum_cent(&SummaryFilter {
            archived: Some(true),
            ..Default::default()
        }),
        200_000 * 100
    );
    assert_eq!(
        sum_cent(&SummaryFilter {
            archived: Some(false),
            ..Default::default()
        }),
        400_000 * 100
    );

    // 类型筛选：两份都是服务类；查「设计」应一无所获
    assert_eq!(
        sum_cent(&SummaryFilter {
            contract_type: Some("SERVICE".into()),
            ..Default::default()
        }),
        600_000 * 100
    );
    assert_eq!(
        sum_cent(&SummaryFilter {
            contract_type: Some("DESIGN".into()),
            ..Default::default()
        }),
        0
    );

    // 关键词：项目名 / 合同名都能命中；完全不相关的词为空
    assert_eq!(
        sum_cent(&SummaryFilter {
            keyword: "测试".into(),
            ..Default::default()
        }),
        600_000 * 100
    );
    assert_eq!(
        sum_cent(&SummaryFilter {
            keyword: "毫不相干".into(),
            ..Default::default()
        }),
        0
    );

    // 日期闭区间：拨款都记在 2026-03-31
    assert_eq!(
        sum_cent(&SummaryFilter {
            from: "2026-04-01".into(),
            to: "2026-12-31".into(),
            ..Default::default()
        }),
        0
    );
    assert_eq!(
        sum_cent(&SummaryFilter {
            from: "2026-01-01".into(),
            to: "2026-03-31".into(),
            ..Default::default()
        }),
        600_000 * 100
    );
}

// ---------------------------------------------------------------- R-46 导出回归

/// 读回 xlsx 单元格文字，用来核对真正写进文件的表头 / 值。
fn xlsx_text(d: Option<&calamine::Data>) -> String {
    match d {
        Some(calamine::Data::String(s)) => s.clone(),
        Some(calamine::Data::Float(f)) => f.to_string(),
        Some(calamine::Data::Int(i)) => i.to_string(),
        Some(calamine::Data::Empty) | None => String::new(),
        Some(other) => format!("{other:?}"),
    }
}

/// R-46：合同台账导出选列后，Excel 只含所选字段，列名与顺序与选择一致；
/// 数据行仍取自同一份 `ReportResult`（筛选 / 排序后的结果）。
#[test]
fn 合同台账导出选列只写所选字段且顺序一致() {
    use calamine::Reader;
    use ctp_app::report::ReportKind;
    let (mut conn, cid) = fixture(1_000_000);
    pay(&mut conn, &cid, 100_000).unwrap();

    let result = service::report_of(&conn, ReportKind::ContractLedger).unwrap();
    assert_eq!(result.columns.len(), 9, "完整台账仍是 9 列");
    let picked = result
        .select_columns(&["project_status", "contract_name", "paid_amount"])
        .unwrap();
    assert_eq!(picked.rows.len(), result.rows.len());

    let path = tmp_xlsx("contract_select");
    let (rows, cols) = service::export_result(&conn, &picked, &path, "t").unwrap();
    assert_eq!(rows, 1);
    assert_eq!(cols, 3);

    let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
    let range = wb.worksheet_range("合同台账").unwrap();
    assert_eq!(xlsx_text(range.get_value((2, 0))), "项目状态");
    assert_eq!(xlsx_text(range.get_value((2, 1))), "合同名称");
    assert_eq!(xlsx_text(range.get_value((2, 2))), "累计已付（元）");
    assert_eq!(xlsx_text(range.get_value((2, 3))), "", "不应有第 4 列");
    assert_eq!(xlsx_text(range.get_value((3, 1))), "测试合同");
    let _ = std::fs::remove_file(&path);
}

/// R-46：拨款一览导出按 `contract_id` 补齐合同名称 / 承包单位，
/// 且与原有导出链路（保存结果、审计日志、列数）兼容。
#[test]
fn 拨款一览导出补齐合同名称与承包单位() {
    use calamine::Reader;
    use ctp_app::report::ReportKind;
    let (mut conn, cid) = fixture(1_000_000);
    pay(&mut conn, &cid, 100_000).unwrap();

    let result = service::report_of(&conn, ReportKind::PaymentLedger).unwrap();
    assert_eq!(result.columns.len(), 7);
    assert_eq!(result.columns[5].title, "合同名称");
    assert_eq!(result.columns[6].title, "承包单位");
    assert_eq!(result.rows[0][5].display(), "测试合同");
    assert_eq!(result.rows[0][6].display(), "某施工单位");

    let path = tmp_xlsx("payment_fields");
    let (rows, cols) =
        service::export_report(&conn, ReportKind::PaymentLedger, &path, "t").unwrap();
    assert_eq!((rows, cols), (1, 7));
    let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
    let range = wb.worksheet_range("拨款一览").unwrap();
    assert_eq!(xlsx_text(range.get_value((2, 5))), "合同名称");
    assert_eq!(xlsx_text(range.get_value((2, 6))), "承包单位");
    assert_eq!(xlsx_text(range.get_value((3, 5))), "测试合同");
    assert_eq!(xlsx_text(range.get_value((3, 6))), "某施工单位");
    let _ = std::fs::remove_file(&path);

    // 两个导出入口共用写文件 + 日志实现，EXPORT 留痕次数不变
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sys_audit_log WHERE action = 'EXPORT' \
             AND target_id = 'PAYMENT_LEDGER_V1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

/// R-46：查不到合同信息时，两列留空且导出照常完成。
#[test]
fn 拨款一览导出合同字段缺失留空不影响导出() {
    use ctp_app::report::{PaymentContractLookup, ReportResult};
    let (conn, _cid) = fixture(1_000_000);
    let row = ctp_core::payment::Payment {
        id: "PM-X".into(),
        payment_no: "BK-X".into(),
        contract_id: "CT-NOT-IN-LOOKUP".into(),
        amount_cent: Money::from_cent(123_456),
        ..Default::default()
    };
    let empty: PaymentContractLookup = PaymentContractLookup::new();
    let result = ReportResult::payment_ledger_export(&[row], "全部合同", &empty);
    assert_eq!(result.rows[0][5].display(), "");
    assert_eq!(result.rows[0][6].display(), "");
    let path = tmp_xlsx("payment_missing");
    let (rows, cols) = service::export_result(&conn, &result, &path, "t").unwrap();
    assert_eq!((rows, cols), (1, 7));
    let _ = std::fs::remove_file(&path);
}

/// R-50：信息修改只落主档字段，编号 / 金额 / 状态保持不动，版本自增并写审计。
#[test]
fn 信息修改只动主档并写审计留痕() {
    let (mut conn, cid) = fixture(1_000_000);
    let before = repo::get_contract(&conn, &cid).unwrap();

    let edited = service::update_contract_master(
        &mut conn,
        &cid,
        EditContract {
            contract_name: "测试合同（更名）".into(),
            project_text: "XJ 测试项目".into(),
            contractor_text: "某施工单位".into(),
            party_a: "某建设单位".into(),
            contract_type: before.contract_type,
            contract_category_code: "GC".into(),
            sign_date: "2026-01-10".into(),
            duration_days: 120,
            planned_start: Some("2026-03-01".into()),
            planned_end: Some("2026-06-28".into()),
            pricing_method: before.pricing_method,
            retention_rate_bp: 500,
            payment_terms: "按月计量支付".into(),
            remark: Some("补充备注".into()),
            bid_method: None,
            open_bid_date: None,
            project_leader: Some("张三".into()),
            contact_info: None,
            bid_agent_text: None,
        },
        "合同名称笔误更正",
        "t",
    )
    .unwrap();

    let changes = edited.changes.clone();
    let after = edited.contract;
    assert_eq!(after.contract_name, "测试合同（更名）");
    assert_eq!(after.duration_days, 120);
    assert_eq!(after.planned_end.as_deref(), Some("2026-06-28"));
    assert_eq!(after.retention_rate_bp, 500);
    assert_eq!(after.project_id, "PJ-1", "按文本回解析到同一档案");
    assert_eq!(after.contractor_id, "ORG-1");
    // 不在修改范围的字段一律原样
    assert_eq!(after.contract_no, before.contract_no, "编号不可变");
    assert_eq!(
        after.original_amount_cent, before.original_amount_cent,
        "金额不在此改"
    );
    assert_eq!(after.status, before.status, "状态不在此改");
    assert_eq!(after.version, before.version + 1, "乐观锁版本应自增");

    // R-51：返回的变更清单是成功弹窗与审计日志的共同来源
    assert!(
        changes.iter().any(|c| c.field == "合同名称"),
        "变更清单必须含合同名称：{changes:?}"
    );
    assert!(
        !changes.iter().any(|c| c.field == "所属项目"),
        "指向同一档案时不该记成项目变更：{changes:?}"
    );
    let long = changes
        .iter()
        .find(|c| c.field == "备注")
        .expect("备注有变，应在清单里");
    assert!(long.long_text, "备注属于长文本，审计只记「已修改」");
    assert_eq!(long.audit_text(), "备注：已修改");

    let detail: String = conn
        .query_row(
            "SELECT detail FROM sys_audit_log WHERE action='EDIT_CONTRACT' ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(detail.contains("合同名称笔误更正"), "必须带原因：{detail}");
    assert!(detail.contains("合同名称"), "必须记字段级变化：{detail}");
    assert!(detail.contains("工期"), "必须记字段级变化：{detail}");
}

/// R-50：空原因、原样提交都要被拒绝，且不留审计噪声。
#[test]
fn 信息修改拒绝空原因与无改动() {
    let (mut conn, cid) = fixture(1_000_000);
    let c = repo::get_contract(&conn, &cid).unwrap();
    let same = || EditContract {
        contract_name: c.contract_name.clone(),
        project_text: "XAJT".into(),
        contractor_text: "某施工单位".into(),
        party_a: c.party_a.clone(),
        contract_type: c.contract_type,
        contract_category_code: c.contract_category_code.clone(),
        sign_date: c.sign_date.clone(),
        duration_days: c.duration_days,
        planned_start: c.planned_start.clone(),
        planned_end: c.planned_end.clone(),
        pricing_method: c.pricing_method,
        retention_rate_bp: c.retention_rate_bp,
        payment_terms: c.payment_terms.clone(),
        remark: c.remark.clone(),
        bid_method: c.bid_method,
        open_bid_date: c.open_bid_date.clone(),
        project_leader: c.project_leader.clone(),
        contact_info: c.contact_info.clone(),
        bid_agent_text: None,
    };

    let err = service::update_contract_master(&mut conn, &cid, same(), "   ", "t").unwrap_err();
    assert!(
        format!("{err}").contains("修改原因"),
        "空原因必须拒绝：{err}"
    );

    let err = service::update_contract_master(&mut conn, &cid, same(), "检查", "t").unwrap_err();
    assert!(
        format!("{err}").contains("没有修改任何内容"),
        "无改动不应产生流水：{err}"
    );
    assert!(
        !format!("{err}").contains("口径违例"),
        "「没改动」不是口径问题，不该顶违例的帽子：{err}"
    );

    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sys_audit_log WHERE action='EDIT_CONTRACT'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "被拒绝的提交不能留审计");
}

/// 造一份「除了指定字段外全都沿用库里的值」的修改入参。
fn edit_like(c: &ctp_core::contract::Contract, project_text: &str) -> EditContract {
    EditContract {
        contract_name: c.contract_name.clone(),
        project_text: project_text.into(),
        contractor_text: "某施工单位".into(),
        party_a: c.party_a.clone(),
        contract_type: c.contract_type,
        contract_category_code: c.contract_category_code.clone(),
        sign_date: c.sign_date.clone(),
        duration_days: c.duration_days,
        planned_start: c.planned_start.clone(),
        planned_end: c.planned_end.clone(),
        pricing_method: c.pricing_method,
        retention_rate_bp: c.retention_rate_bp,
        payment_terms: c.payment_terms.clone(),
        remark: c.remark.clone(),
        bid_method: c.bid_method,
        open_bid_date: c.open_bid_date.clone(),
        project_leader: c.project_leader.clone(),
        contact_info: c.contact_info.clone(),
        bid_agent_text: None,
    }
}

/// 再加一个项目档案：合同改项目时用。
fn add_project(conn: &Connection, id: &str, code: &str, name: &str) {
    let now = ctp_core::date::now_iso();
    repo::insert_project(
        conn,
        &Project {
            id: id.into(),
            project_code: code.into(),
            project_name: name.into(),
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

/// R-51：修改所属项目时名称优先。
///
/// 用户把「XAJT 测试项目」改成「XAJT 另一个项目」（旧编码留在框里），
/// 必须解析到名称段指向的项目，而不是退回旧编码对应的项目后报「没有修改任何内容」。
#[test]
fn 修改所属项目_名称优先不被旧编码劫持() {
    let (mut conn, cid) = fixture(1_000_000);
    add_project(&conn, "PJ-2", "PRJ-2026-002", "另一个项目");
    let before = repo::get_contract(&conn, &cid).unwrap();

    let edited = service::update_contract_master(
        &mut conn,
        &cid,
        edit_like(&before, "XAJT 另一个项目"),
        "项目归属更正",
        "t",
    )
    .unwrap();

    assert_eq!(edited.contract.project_id, "PJ-2", "应按名称段解析到新项目");
    let row = edited
        .changes
        .iter()
        .find(|c| c.field == "所属项目")
        .expect("项目变了必须进变更清单");
    assert_eq!(row.old, "测试项目", "弹窗里的原值用项目名称，不带编码");
    assert_eq!(row.new, "另一个项目");

    // 只写名称、只写编码也都应命中同一条档案
    for text in ["另一个项目", "PRJ-2026-002", "PRJ-2026-002 另一个项目"] {
        let before = repo::get_contract(&conn, &cid).unwrap();
        let edited =
            service::update_contract_master(&mut conn, &cid, edit_like(&before, text), "核对", "t");
        match edited {
            Ok(e) => assert_eq!(e.contract.project_id, "PJ-2", "「{text}」应指向 PJ-2"),
            // 已经指向 PJ-2 时再提交同值会走「没有修改任何内容」，同样说明解析正确
            Err(e) => assert!(
                format!("{e}").contains("没有修改任何内容"),
                "「{text}」不应报其他错误：{e}"
            ),
        }
    }
}

/// R-51：编码命中但名称被改成新名字时，按名称段新建档案，不静默回退旧项目。
#[test]
fn 修改所属项目_名称改成新名字按新名称建档() {
    let (mut conn, cid) = fixture(1_000_000);
    let before = repo::get_contract(&conn, &cid).unwrap();

    let edited = service::update_contract_master(
        &mut conn,
        &cid,
        edit_like(&before, "XAJT 二期工程"),
        "项目拆分",
        "t",
    )
    .unwrap();

    assert_ne!(
        edited.contract.project_id, "PJ-1",
        "不能退回旧编码对应的项目"
    );
    let projects = repo::list_projects(&conn).unwrap();
    let p = projects
        .iter()
        .find(|p| p.id == edited.contract.project_id)
        .expect("应已建档");
    assert_eq!(p.project_name, "二期工程", "编码前缀不该进项目名称");
    assert!(p.project_code.starts_with("PRJ-"), "新档案自动编码");
    assert_eq!(
        edited
            .changes
            .iter()
            .find(|c| c.field == "所属项目")
            .map(|c| c.new.as_str()),
        Some("二期工程")
    );
}

// ---------------------------------------------------------------- R-52

fn action_count(conn: &Connection, action: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sys_audit_log WHERE action = ?1",
        [action],
        |r| r.get(0),
    )
    .unwrap()
}

/// R-52 / P2·P3·P4：金额、审计价、交工日期重复提交 —— 不写历史、不写日志，
/// 也不能顶「口径违例」的帽子（那不是违规，只是没变化）。
#[test]
fn 同值提交不写历史也不顶口径违例() {
    let (mut conn, cid) = fixture(1_000_000);

    // —— 合同金额：与库里完全一致
    let c = repo::get_contract(&conn, &cid).unwrap();
    let err = service::edit_contract_amount(&mut conn, &cid, c.original_amount_cent, "复核", "t")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("金额未变化"), "{msg}");
    assert!(!msg.contains("口径违例"), "「没变化」不是口径问题：{msg}");

    // —— 审计价：先登记，再原样提交（弹窗预填的就是现值）
    service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(90_000_000)),
        Some("2026-03-01".into()),
        Some("某造价咨询".into()),
        Some("SH-001".into()),
        "结算审定",
        "t",
    )
    .unwrap();
    let logs_before = action_count(&conn, "SET_AUDIT_PRICE");
    let hist_before = ctp_app::history::load(&conn, ctp_app::history::KEY_AUDIT_PRICE)
        .unwrap()
        .len();
    let err = service::register_audit_price(
        &mut conn,
        &cid,
        Some(Money::from_cent(90_000_000)),
        Some("2026-03-01".into()),
        Some("某造价咨询".into()),
        Some("SH-001".into()),
        "复核",
        "t",
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("审计价信息未变化"), "{msg}");
    assert!(!msg.contains("口径违例"), "「没变化」不是口径问题：{msg}");
    assert_eq!(
        action_count(&conn, "SET_AUDIT_PRICE"),
        logs_before,
        "同值不该多写一条日志"
    );
    assert_eq!(
        ctp_app::history::load(&conn, ctp_app::history::KEY_AUDIT_PRICE)
            .unwrap()
            .len(),
        hist_before,
        "同值不该多写一条审计价历史"
    );

    // —— 交工日期：fixture 已登记 2020-01-01
    let logs_before = action_count(&conn, "SET_DELIVERY_DATE");
    let err = service::set_delivery_date(&mut conn, &cid, "2020-01-01", "t").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("交工日期未变化"), "{msg}");
    assert!(!msg.contains("口径违例"), "「没变化」不是口径问题：{msg}");
    assert_eq!(
        action_count(&conn, "SET_DELIVERY_DATE"),
        logs_before,
        "同值不该多写一条日志"
    );
}

/// R-52 / P6：单位文本带编码前缀时按名称段认领，不再建出「ORG-1 某施工单位」
/// 这样的重复档案（系统没有档案维护界面，重复项无法合并）。
#[test]
fn 承包单位写成编码加名称不重复建档() {
    let (mut conn, _cid) = fixture(1_000_000);
    let org_count = repo::list_contractors(&conn).unwrap().len();

    let mk = |name: &str, contractor_text: &str| NewContract {
        project_text: Some("测试项目".into()),
        contractor_text: Some(contractor_text.to_string()),
        contract_name: name.into(),
        contract_type: ctp_core::contract::ContractType::Service,
        contract_category_code: "GC".into(),
        party_a: "某建设单位".into(),
        sign_date: "2026-02-01".into(),
        original_amount: Money::from_cent(10_000_000),
        payment_terms: "按月计量支付".into(),
        ..Default::default()
    };

    // 已存在的单位：整串里带编码，仍应复用 ORG-1，不新增档案
    let c =
        service::create_contract(&mut conn, mk("编码前缀复用", "ORG-1 某施工单位"), "t").unwrap();
    assert_eq!(c.contractor_id, "ORG-1", "编码前缀不该绕开已有档案");
    assert_eq!(
        repo::list_contractors(&conn).unwrap().len(),
        org_count,
        "单位档案数量不应增加"
    );

    // 新单位：建档时只写名称段，前缀不进入单位名称
    let c2 = service::create_contract(&mut conn, mk("编码前缀新建", "ORG-9 新单位"), "t").unwrap();
    let orgs = repo::list_contractors(&conn).unwrap();
    let made = orgs
        .iter()
        .find(|o| o.id == c2.contractor_id)
        .expect("应已建档");
    assert_eq!(made.contractor_name, "新单位", "编码前缀不该进单位名称");
    assert_eq!(orgs.len(), org_count + 1, "只应新增一条");
}
