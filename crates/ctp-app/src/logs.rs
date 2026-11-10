//! 操作日志的应用层：查询、类型/对象的中文映射、清理留痕。
//!
//! 日志本身由各业务入口在**同一事务内**写入（`db::insert_audit_log`），这里只负责
//! 读出来给人看、以及按条件清理。清理动作自身也留一条 `CLEAR`，否则日志被谁清空无从追溯。

use ctp_store::audit::{self, AuditLogRow, LogFilter};
use ctp_store::{db, Result};
use rusqlite::Connection;

pub use ctp_store::audit::AUDIT_KEEP_MAX;

/// 操作类型 → 中文。未登记的类型原样返回，新增埋点忘了映射也能看见。
pub fn action_label(action: &str) -> String {
    let name = match action {
        "CREATE" => "新建",
        "ACTIVATE" => "生效",
        "VOID" => "作废",
        "EDIT_AMOUNT" => "修改金额",
        "SET_AUDIT_PRICE" => "登记审计价",
        "SET_DELIVERY_DATE" => "登记交工日期",
        "REGISTER" => "登记拨款",
        "UPSERT" => "上传/替换附件",
        "DELETE" => "删除附件",
        "EXPORT" => "导出报表",
        "IMPORT_START" => "批量导入开始",
        "IMPORT_DONE" => "批量导入结束",
        "LOGIN_OK" => "管理登录成功",
        "LOGIN_FAIL" => "管理登录失败",
        "CHANGE_PASSWORD" => "修改管理密码",
        "SET_TOLERANCE" => "修改超付容差",
        "CLEAR" => "清理日志",
        other => other,
    };
    name.to_string()
}

/// 对象类型 → 中文。
pub fn target_label(target_type: &str) -> String {
    let name = match target_type {
        "ct_contract" => "合同",
        "pay_payment" => "拨款单",
        "ct_contract_attachment" => "合同附件",
        "prj_project" => "项目",
        "org_contractor" => "单位",
        "report" => "报表",
        "import" => "批量导入",
        "session" => "登录会话",
        "sys_meta" => "系统设置",
        "sys_audit_log" => "操作日志",
        other => other,
    };
    name.to_string()
}

pub fn list(conn: &Connection, filter: &LogFilter, limit: i64) -> Result<Vec<AuditLogRow>> {
    audit::list(conn, filter, limit)
}

pub fn count(conn: &Connection, filter: &LogFilter) -> Result<i64> {
    audit::count(conn, filter)
}

pub fn total(conn: &Connection) -> Result<i64> {
    audit::total(conn)
}

/// 出现过的操作类型（界面筛选项）。
pub fn actions(conn: &Connection) -> Result<Vec<String>> {
    audit::actions(conn)
}

/// 「实质全清」至少保留的最近条数（R-68 / C14-A）。
///
/// 全删不可恢复，误触一次就把审计链清零；保底不再只看「条件是否全空」，
/// 而是看删完后还剩多少——条件拉满或关键词覆盖全表同样要兜住。
pub use ctp_store::audit::AUDIT_KEEP_MIN;

/// 清理日志并留痕，返回删除条数。
///
/// `filter` 四个条件全空即「全部清理」；否则按条件清理（例如只清某个时间范围）。
/// R-39：删除与 `CLEAR` 留痕放在同一个 IMMEDIATE 事务里，
/// 否则「删光了但留痕写入失败」会得到一个没有来源说明的空日志表。
/// 保底按「删完后还剩多少」判定（见 `audit::needs_floor`），任何形式的
/// 实质全清都至少留下最近 [`AUDIT_KEEP_MIN`] 条，留痕文案与实际行为一致。
pub fn clear(conn: &mut Connection, filter: &LogFilter, operator: &str) -> Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let keep_min = audit::needs_floor(&tx, filter, filter.is_empty())?;
    let n = audit::clear(&tx, filter, keep_min)?;
    let scope = describe(filter);
    let kept_note = if keep_min {
        format!("，保留最近 {AUDIT_KEEP_MIN} 条")
    } else {
        String::new()
    };
    db::insert_audit_log(
        &tx,
        "sys_audit_log",
        "sys_audit_log",
        Some("操作日志"),
        "CLEAR",
        Some(&format!("{scope}，共 {n} 条{kept_note}")),
        operator,
    )?;
    tx.commit()?;
    Ok(n)
}

/// 把筛选条件写成一句人话，供清理留痕与界面提示共用。
pub fn describe(filter: &LogFilter) -> String {
    let mut parts: Vec<String> = Vec::new();
    let from = filter.from.trim();
    let to = filter.to.trim();
    if !from.is_empty() || !to.is_empty() {
        parts.push(format!(
            "时间 {} ~ {}",
            if from.is_empty() { "不限" } else { from },
            if to.is_empty() { "不限" } else { to }
        ));
    }
    if let Some(a) = filter.action.as_ref().filter(|s| !s.trim().is_empty()) {
        parts.push(format!("类型 {}", action_label(a)));
    }
    let kw = filter.keyword.trim();
    if !kw.is_empty() {
        parts.push(format!("关键词「{kw}」"));
    }
    if parts.is_empty() {
        "全部清理".to_string()
    } else {
        format!("按条件清理（{}）", parts.join("、"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        db::init_schema(&mut conn).unwrap();
        conn
    }

    fn add(conn: &Connection, action: &str, label: &str) {
        db::insert_audit_log(
            conn,
            "ct_contract",
            "CT-1",
            Some(label),
            action,
            Some("明细"),
            "管理员",
        )
        .unwrap();
    }

    #[test]
    fn 类型与对象的中文映射() {
        assert_eq!(action_label("REGISTER"), "登记拨款");
        assert_eq!(action_label("SOMETHING_NEW"), "SOMETHING_NEW");
        assert_eq!(target_label("pay_payment"), "拨款单");
        assert_eq!(target_label("ct_contract_attachment"), "合同附件");
    }

    #[test]
    fn 全部清理保留最近两百条并留痕() {
        let mut conn = fixture();
        for i in 0..205 {
            add(&conn, "CREATE", &format!("HT-00{i} 合同"));
        }
        let n = clear(&mut conn, &LogFilter::default(), "管理员").unwrap();
        // 205 条只删掉最旧的 5 条，最近 200 条保底保留
        assert_eq!(n, 5);
        assert_eq!(total(&conn).unwrap(), 201); // 200 条 + 1 条留痕
        let rows = list(&conn, &LogFilter::default(), 10).unwrap();
        assert_eq!(rows[0].action, "CLEAR");
        let detail = rows[0].detail.as_deref().unwrap();
        assert!(detail.contains("全部清理"), "{detail}");
        assert!(detail.contains("保留最近 200 条"), "{detail}");
        assert!(detail.contains("5 条"), "{detail}");
    }

    #[test]
    fn 按条件清理的留痕写明范围() {
        let mut conn = fixture();
        add(&conn, "CREATE", "HT-001 道路改造");
        add(&conn, "VOID", "HT-002 桥梁维修");
        let f = LogFilter {
            action: Some("VOID".into()),
            ..Default::default()
        };
        assert_eq!(clear(&mut conn, &f, "管理员").unwrap(), 1);
        let rows = list(&conn, &LogFilter::default(), 10).unwrap();
        let clear_row = rows.iter().find(|r| r.action == "CLEAR").unwrap();
        let detail = clear_row.detail.as_deref().unwrap();
        assert!(detail.contains("作废"), "留痕应写明类型：{detail}");
    }

    #[test]
    fn 条件描述对空条件说全部清理() {
        assert_eq!(describe(&LogFilter::default()), "全部清理");
    }
}
