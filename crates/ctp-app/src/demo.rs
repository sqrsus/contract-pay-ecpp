use crate::service::{self, NewContract, NewPayment};
use ctp_core::date::today_iso;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_core::project::{Contact, Contractor, ContractorRole, Project};
use ctp_store::{repo, Result};
use rusqlite::Connection;

fn yuan(v: i64) -> Money {
    Money::from_cent(v * 100)
}

/// 演示数据：覆盖「未审定 / 已审定 / 已作废一笔」三种典型形态，
/// 便于用 `summary`、`ledger`、`category` 三个命令直接核对口径。
pub fn seed_demo(conn: &mut Connection, operator: &str) -> Result<()> {
    ctp_store::seed::seed_reference(conn, operator)?;

    let now = ctp_core::date::now_iso();
    repo::insert_project(
        conn,
        &Project {
            id: "PJ-1".into(),
            project_code: "XAJT".into(),
            project_name: "某市政道路改扩建工程".into(),
            remark: Some("演示项目".into()),
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some(operator.into()),
            updated_at: now.clone(),
            updated_by: Some(operator.into()),
        },
    )?;

    repo::insert_contractor(
        conn,
        &Contractor {
            id: "ORG-1".into(),
            contractor_name: "某建工集团有限公司".into(),
            role_type: ContractorRole::Construct,
            credit_code: Some("91610000XXXXXXXXXX".into()),
            bank_name: Some("某银行营业部".into()),
            bank_account: Some("6222********1234".into()),
            remark: None,
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some(operator.into()),
            updated_at: now.clone(),
            updated_by: Some(operator.into()),
        },
    )?;
    repo::insert_contractor(
        conn,
        &Contractor {
            id: "ORG-2".into(),
            contractor_name: "某工程监理有限公司".into(),
            role_type: ContractorRole::Supervise,
            credit_code: None,
            bank_name: None,
            bank_account: None,
            remark: None,
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some(operator.into()),
            updated_at: now.clone(),
            updated_by: Some(operator.into()),
        },
    )?;
    repo::insert_contractor(
        conn,
        &Contractor {
            id: "ORG-3".into(),
            contractor_name: "某建材有限公司".into(),
            role_type: ContractorRole::Supplier,
            credit_code: None,
            bank_name: None,
            bank_account: None,
            remark: None,
            is_deleted: false,
            version: 1,
            created_at: now.clone(),
            created_by: Some(operator.into()),
            updated_at: now.clone(),
            updated_by: Some(operator.into()),
        },
    )?;
    repo::insert_contact(
        conn,
        &Contact {
            id: "CT-ORG-1-1".into(),
            contractor_id: "ORG-1".into(),
            contact_name: "张工".into(),
            phone: Some("13800000000".into()),
            is_primary: true,
            version: 1,
            created_at: now.clone(),
            created_by: Some(operator.into()),
            updated_at: now.clone(),
            updated_by: Some(operator.into()),
        },
    )?;

    // 合同一：施工总承包，未审定，两笔拨款
    let c1 = service::create_contract(
        conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "道路工程（一标段）施工总承包".into(),
            contract_category_code: "GC".into(),
            category_id: Some("C02".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-15".into(),
            original_amount: yuan(2_850_000),
            duration_days: 365,
            planned_start: Some("2026-02-01".into()),
            planned_end: Some("2027-01-31".into()),
            retention_rate_bp: 300,
            advance_rate_bp: 1_000,
            payment_terms: "按月计量支付，进度款按审定量的 80% 拨付，质保金 3%。".into(),
            remark: None,
            ..Default::default()
        },
        operator,
    )?;
    service::activate_contract(conn, &c1.id, operator)?;

    let mut p = NewPayment::new(&c1.id, yuan(1_000_000), "2026-03-31");
    p.reason = Some("2026 年 3 月进度款".into());
    service::register_payment(conn, p, operator)?;

    let mut p = NewPayment::new(&c1.id, yuan(600_000), "2026-06-30");
    p.reason = Some("2026 年 6 月进度款".into());
    service::register_payment(conn, p, operator)?;

    // 合同二：监理服务，已审定（审计价低于合同价），一笔拨款
    let c2 = service::create_contract(
        conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "道路工程施工监理服务".into(),
            contract_type: ctp_core::contract::ContractType::Supervise,
            contract_category_code: "FW".into(),
            category_id: Some("C04".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-2".into(),
            sign_date: "2026-01-20".into(),
            original_amount: yuan(120_000),
            duration_days: 365,
            planned_start: Some("2026-02-01".into()),
            planned_end: Some("2027-01-31".into()),
            payment_terms: "按季度支付，竣工后结清。".into(),
            ..Default::default()
        },
        operator,
    )?;
    service::activate_contract(conn, &c2.id, operator)?;
    service::register_audit_price(
        conn,
        &c2.id,
        Some(yuan(118_000)),
        Some("2026-10-20".into()),
        Some("某造价咨询公司".into()),
        Some("SJ-2026-0088".into()),
        "竣工结算审计完成，按审定金额登记",
        operator,
    )?;
    let mut p = NewPayment::new(&c2.id, yuan(30_000), "2026-04-30");
    p.reason = Some("第一季度监理费".into());
    service::register_payment(conn, p, operator)?;

    // 合同三：材料采购，一笔拨款作废（演示 H5 的「作废 + 重登」）
    let c3 = service::create_contract(
        conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "沥青混合料采购".into(),
            contract_type: ctp_core::contract::ContractType::Supply,
            contract_category_code: "CL".into(),
            category_id: Some("C03".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-3".into(),
            sign_date: "2026-02-10".into(),
            original_amount: yuan(500_000),
            duration_days: 180,
            payment_terms: "按供货批次月结。".into(),
            ..Default::default()
        },
        operator,
    )?;
    service::activate_contract(conn, &c3.id, operator)?;
    let mut p = NewPayment::new(&c3.id, yuan(200_000), "2026-05-31");
    p.reason = Some("首批供货（金额录错，稍后作废重登）".into());
    let wrong = service::register_payment(conn, p, operator)?;
    service::void_payment(conn, &wrong.id, "金额录入错误，作废后按实际 120,000 元重登", operator)?;

    let mut p = NewPayment::new(&c3.id, yuan(120_000), "2026-05-31");
    p.reason = Some("首批供货（重登）".into());
    p.pay_method = PayMethod::Transfer;
    service::register_payment(conn, p, operator)?;

    println!("演示数据已写入（基准日期 {}）", today_iso());
    Ok(())
}
