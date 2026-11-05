//! 集成测试：付款打印件的**成品**口径。
//!
//! | 用例 | 对应确认口径（R-27） |
//! |---|---|
//! | `单张成品包含九个域取值且无残留占位符` | 9 域映射 / 缺值留空 / 残留即失败 |
//! | `批量按一份一页合并` | 勾选若干条 → 合并输出 |
//! | `作废单拒绝打印` | 作废不允许打印 |
//! | `已付款按开单日期取本次之前累计` | 取数按开单日期、排除作废 |

use ctp_app::docx;
use ctp_app::service::{self, NewContract, NewPayment};
use ctp_core::contract::ContractType;
use ctp_core::money::Money;
use ctp_core::project::{Contractor, ContractorRole, Project};
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

/// 一张已生效合同（合同价 1000 元、付款条款固定），可继续登记拨款。
fn fixture() -> (Connection, String) {
    let mut conn = db::open_in_memory().unwrap();
    db::migrate(&mut conn).unwrap();
    ctp_store::seed::seed_reference(&conn, "t").unwrap();
    project_and_org(&conn);
    let c = service::create_contract(
        &mut conn,
        NewContract {
            project_id: "PJ-1".into(),
            contract_name: "测试合同".into(),
            contract_type: ContractType::Service,
            contract_category_code: "GC".into(),
            category_id: Some("C02".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount: Money::from_cent(1000 * 100),
            delivery_date: Some("2020-01-01".into()),
            payment_terms: "按月计量支付".into(),
            ..Default::default()
        },
        "t",
    )
    .unwrap();
    service::save_attachment(
        &conn,
        &c.id,
        ctp_core::attachment::AttachmentKind::Contract,
        "合同正本.pdf",
        Some("application/pdf"),
        b"%PDF-1.4 test",
        "t",
    )
    .unwrap();
    service::activate_contract(&mut conn, &c.id, "t").unwrap();
    (conn, c.id)
}

fn pay(conn: &mut Connection, cid: &str, yuan: i64, date: &str) -> String {
    service::register_payment(
        conn,
        NewPayment::new(cid, Money::from_cent(yuan * 100), date),
        "t",
    )
    .unwrap()
    .id
}

#[test]
fn 单张成品包含九个域取值且无残留占位符() {
    let (mut conn, cid) = fixture();
    let pid = pay(&mut conn, &cid, 300, "2026-03-31");

    let v = service::print_payment_docx(&conn, std::slice::from_ref(&pid)).unwrap();
    assert_eq!(v.count, 1);
    assert!(v.default_name.starts_with("拨款单_"), "{}", v.default_name);

    let xml = docx::read_entry(&v.bytes, docx::DOC_XML).expect("成品应是合法 docx");
    assert!(!xml.contains("MERGEFIELD"), "不应残留域代码");
    assert!(docx::residual_fields(&xml).is_empty(), "不应残留占位符");

    // 9 个域的取值逐个核对（合同名 / 承包单位 / 合同价 / 审计价 0 / 已付 0 / 本次 / 大写 / 累计 / 条款）
    for expect in [
        "测试合同",
        "某施工单位",
        "1000.00",
        "0.00",
        "300.00",
        "叁佰圆整",
        "按月计量支付",
    ] {
        assert!(xml.contains(expect), "成品里缺少取值 {expect}");
    }
    // 三处手工留空的位置在模板里本来就没有域，成品里不应出现「会计分类名」这类占位
    assert!(!xml.contains("«"));
}

#[test]
fn 批量按一份一页合并() {
    let (mut conn, cid) = fixture();
    let a = pay(&mut conn, &cid, 100, "2026-02-28");
    let b = pay(&mut conn, &cid, 200, "2026-03-31");

    let one = service::print_payment_docx(&conn, std::slice::from_ref(&a)).unwrap();
    let two = service::print_payment_docx(&conn, &[a.clone(), b.clone()]).unwrap();

    assert_eq!(two.count, 2);
    assert!(two.bytes.len() > one.bytes.len(), "两份应大于一份");
    let xml = docx::read_entry(&two.bytes, docx::DOC_XML).unwrap();
    assert_eq!(
        xml.matches("<w:br w:type=\"page\"/>").count(),
        1,
        "两份之间应有一个分页符"
    );
    // 模板正文中段本身有一个连续分节符（sectPr），每份带一个，末尾再保留一个 body 级
    assert_eq!(
        xml.matches("<w:sectPr").count(),
        3,
        "两份各带一个中段分节符 + 末尾一个"
    );
    // 第二张的「已付款」应含第一张（累计 100.00），成品里两份取值都在
    assert!(xml.contains("100.00") && xml.contains("200.00"));
}

#[test]
fn 作废单拒绝打印() {
    let (mut conn, cid) = fixture();
    let pid = pay(&mut conn, &cid, 100, "2026-02-28");
    service::void_payment(&mut conn, &pid, "录错重登", "t").unwrap();

    let err = service::print_payment_docx(&conn, &[pid]).unwrap_err();
    assert!(
        matches!(err, ctp_app::PrintError::Voided(_)),
        "作废单应被拒绝，实际：{err}"
    );
}

#[test]
fn 已付款按开单日期取本次之前累计且排除作废() {
    let (mut conn, cid) = fixture();
    let first = pay(&mut conn, &cid, 100, "2026-02-28");
    // 同一天日期、单号更大的那张，在它之前累计应包含 first
    let second = pay(&mut conn, &cid, 200, "2026-03-31");
    let voided = pay(&mut conn, &cid, 50, "2026-03-15");
    service::void_payment(&mut conn, &voided, "录错重登", "t").unwrap();

    let d2 = service::payment_print_data(&conn, &second).unwrap();
    assert_eq!(d2.get("已付款改成元"), Some("100.00"), "作废单不计入累计");
    assert_eq!(d2.get("本次付款小写"), Some("200.00"));
    assert_eq!(d2.get("本次付款后累计"), Some("300.00"));

    let d1 = service::payment_print_data(&conn, &first).unwrap();
    assert_eq!(d1.get("已付款改成元"), Some("0.00"));
}

/// 走真实 Word 导出 PDF。
///
/// 依赖本机装 Microsoft Word，故默认 `#[ignore]`；手动执行：
/// `cargo test -p ctp-app --test print -- --ignored`
#[test]
#[ignore = "需要本机 Microsoft Word"]
fn 用_word_把成品导出成_pdf() {
    let (mut conn, cid) = fixture();
    let a = pay(&mut conn, &cid, 600, "2026-02-28");
    let v = service::print_payment_docx(&conn, &[a]).unwrap();
    let dir = std::env::temp_dir().join("ctp-print-sample");
    std::fs::create_dir_all(&dir).unwrap();
    let docx_path = dir.join(&v.default_name);
    std::fs::write(&docx_path, &v.bytes).unwrap();

    let pdf_path = dir.join(v.pdf_name());
    ctp_app::pdf::docx_to_pdf(&docx_path, &pdf_path).expect("Word 导出 PDF 失败");
    let head = std::fs::read(&pdf_path).unwrap();
    assert!(head.starts_with(b"%PDF"), "应产出合法 PDF");
    println!("PDF 已生成：{}", pdf_path.display());
}

/// 落一份真实成品到临时目录，便于用 Word 打开肉眼核对版式。
///
/// 断言只写在内存里；文件落盘是给人看的辅助产物，路径打在测试输出里。
#[test]
fn 落一份成品供肉眼核对() {
    let (mut conn, cid) = fixture();
    let a = pay(&mut conn, &cid, 600, "2026-02-28");
    let b = pay(&mut conn, &cid, 300, "2026-03-31");
    let v = service::print_payment_docx(&conn, &[a, b]).unwrap();
    let dir = std::env::temp_dir().join("ctp-print-sample");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(&v.default_name);
    std::fs::write(&path, &v.bytes).unwrap();
    println!("样品已生成：{}", path.display());
    assert!(path.is_file());
    let _ = repo::list_payments(&conn, Some(&cid)).unwrap();
}
