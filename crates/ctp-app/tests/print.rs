//! 集成测试：付款打印的**数据与版面**口径。
//!
//! | 用例 | 对应口径 |
//! |---|---|
//! | `版面数据含九个域取值` | 9 域映射 / 缺值留空 |
//! | `批量按列表顺序一张两页` | 勾选若干条 → 一张两页、连续排页 |
//! | `作废单拒绝打印` | 作废不允许打印 |
//! | `已付款按开单日期取本次之前累计且排除作废` | 取数按开单日期、排除作废 |
//! | `预览渲染两页且版面有实际内容` | 版面能渲染、尺寸为 A4 横向 |
//!
//! R-28 起打印不再经过 docx / Word，故原来针对 docx 字节的断言整体替换为
//! 「版面数据 + 渲染位图」两类断言。

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
        &mut conn,
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
fn 版面数据含九个域取值() {
    let (mut conn, cid) = fixture();
    let pid = pay(&mut conn, &cid, 300, "2026-03-31");

    let vs = service::print_vouchers(&conn, std::slice::from_ref(&pid)).unwrap();
    assert_eq!(vs.len(), 1);
    let v = &vs[0];

    // 9 个域逐个核对（合同名 / 承包单位 / 合同价 / 审计价 0 / 已付 0 / 本次 / 大写 / 累计 / 条款）
    assert_eq!(v.project_name, "测试合同");
    assert_eq!(v.contractor, "某施工单位");
    assert_eq!(v.contract_amount, "1000.00");
    assert_eq!(v.audit_amount, "0.00");
    assert_eq!(v.paid_before, "0.00");
    assert_eq!(v.this_amount, "300.00");
    assert_eq!(v.this_amount_cn, "叁佰圆整");
    assert_eq!(v.cumulative, "300.00");
    assert_eq!(v.terms, "按月计量支付");
    assert!(v.payment_no.starts_with("BK-"), "{}", v.payment_no);
}

#[test]
fn 批量按列表顺序一张两页() {
    let (mut conn, cid) = fixture();
    let a = pay(&mut conn, &cid, 100, "2026-02-28");
    let b = pay(&mut conn, &cid, 200, "2026-03-31");

    let one = service::print_vouchers(&conn, std::slice::from_ref(&a)).unwrap();
    let two = service::print_vouchers(&conn, &[a, b]).unwrap();

    assert_eq!(one.len(), 1);
    assert_eq!(two.len(), 2, "两张单据");
    assert_eq!(ctp_app::print::page_count(two.len()), 4, "每张两页");

    // 第二张的「已付款」含第一张（累计 100.00），说明顺序与取数都对
    assert_eq!(two[0].paid_before, "0.00");
    assert_eq!(two[1].paid_before, "100.00");
    assert_eq!(two[1].cumulative, "300.00");

    let name = ctp_app::print::doc_name(&two);
    assert!(name.contains("等 2 张"), "{name}");
}

#[test]
fn 作废单拒绝打印() {
    let (mut conn, cid) = fixture();
    let pid = pay(&mut conn, &cid, 100, "2026-02-28");
    service::void_payment(&mut conn, &pid, "录错重登", "t").unwrap();

    let err = service::print_vouchers(&conn, &[pid]).unwrap_err();
    assert!(
        matches!(err, ctp_app::PrintError::Voided(_)),
        "作废单应被拒绝，实际：{err}"
    );
}

#[test]
fn 已付款按开单日期取本次之前累计且排除作废() {
    let (mut conn, cid) = fixture();
    let first = pay(&mut conn, &cid, 100, "2026-02-28");
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

/// 渲染出的位图尺寸必须是 A4 横向，且纸上真有内容（不是一张全白）。
#[cfg(windows)]
#[test]
fn 预览渲染两页且版面有实际内容() {
    let (mut conn, cid) = fixture();
    let pid = pay(&mut conn, &cid, 600, "2026-02-28");
    let vs = service::print_vouchers(&conn, &[pid]).unwrap();

    for page in 0..ctp_app::paper::PAGES_PER_VOUCHER {
        let pv = ctp_app::render_print_page(&vs[0], page, 100.0).expect("渲染失败");
        // 100 dpi 下 A4 横向约 1169 × 827
        assert!(
            pv.width > pv.height,
            "必须是横向：{}×{}",
            pv.width,
            pv.height
        );
        assert_eq!(pv.rgba.len(), (pv.width * pv.height * 4) as usize);
        let dark = pv
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 128 && px[1] < 128 && px[2] < 128)
            .count();
        assert!(dark > 5000, "第 {page} 页几乎没有笔迹（dark={dark}）");
        assert!(
            pv.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255),
            "预览必须完全不透明"
        );
    }
}

/// 落两页预览到临时目录（BMP，24 位），便于用图片查看器核对版面。
///
/// 与出纸共用同一份版面代码，所以看上图就等于看上纸。
#[cfg(windows)]
#[test]
fn 落一份预览供肉眼核对() {
    let (mut conn, cid) = fixture();
    let a = pay(&mut conn, &cid, 600, "2026-02-28");
    let b = pay(&mut conn, &cid, 300, "2026-03-31");
    let vs = service::print_vouchers(&conn, &[a, b]).unwrap();
    let dir = std::env::temp_dir().join("ctp-print-preview");
    std::fs::create_dir_all(&dir).unwrap();

    for (i, v) in vs.iter().enumerate() {
        for page in 0..ctp_app::paper::PAGES_PER_VOUCHER {
            let pv = ctp_app::render_print_page(v, page, 150.0).unwrap();
            let path = dir.join(format!("voucher{i}_p{page}.bmp"));
            std::fs::write(&path, to_bmp(&pv.rgba, pv.width, pv.height)).unwrap();
            println!("预览已生成：{}", path.display());
        }
    }
    assert!(dir.join("voucher0_p0.bmp").is_file());
}

/// RGBA8（自上而下）→ 24 位 BMP（自下而上，行按 4 字节对齐）。
#[cfg(windows)]
fn to_bmp(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = (w as usize * 3).div_ceil(4) * 4;
    let size = row * h as usize;
    let mut out = Vec::with_capacity(54 + size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + size) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut line = vec![0u8; row];
    for y in (0..h as usize).rev() {
        for x in 0..w as usize {
            let p = (y * w as usize + x) * 4;
            line[x * 3] = rgba[p + 2];
            line[x * 3 + 1] = rgba[p + 1];
            line[x * 3 + 2] = rgba[p];
        }
        out.extend_from_slice(&line);
        line.iter_mut().for_each(|b| *b = 0);
    }
    out
}
