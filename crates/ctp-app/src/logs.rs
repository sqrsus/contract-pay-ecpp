//! 操作日志的应用层：查询、类型/对象的中文映射、清理留痕。
//!
//! 日志本身由各业务入口在**同一事务内**写入（`db::insert_audit_log`），这里只负责
//! 读出来给人看、以及一次性清空。清空动作自身也留一条 `CLEAR`，否则日志被谁清空无从追溯。

use ctp_store::audit::{self, AuditLogRow, LogFilter};
use ctp_store::{db, Result};
use rusqlite::Connection;

pub use ctp_store::audit::AUDIT_KEEP_MAX;

/// 操作类型 → 中文。未登记的类型原样返回，新增埋点忘了映射也能看见。
pub fn action_label(action: &str) -> String {
    let name = match action {
        "CREATE" => "新建",
        "ACTIVATE" => "生效",
        "REVERT_ACTIVE" => "回退生效",
        "PERFORMING" => "转为履行中",
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

/// 一次性清空时至少保留的最近条数（R-68 / C14-A）。
pub use ctp_store::audit::AUDIT_KEEP_MIN;

/// 一次性清空日志并留痕，返回删除条数。
///
/// R-39：删除与 `CLEAR` 留痕放在同一个 IMMEDIATE 事务里，
/// 否则「删光了但留痕写入失败」会得到一个没有来源说明的空日志表。
/// R-70：不再接收筛选条件，也不再提供「按时间段 / 按条件」清理；
/// 删除固定走 `audit::clear_all`，保底保留最近 [`AUDIT_KEEP_MIN`] 条，
/// 留痕文案与实际行为一致。
pub fn clear(conn: &mut Connection, operator: &str) -> Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let n = audit::clear_all(&tx)?;
    db::insert_audit_log(
        &tx,
        "sys_audit_log",
        "sys_audit_log",
        Some("操作日志"),
        "CLEAR",
        Some(&format!(
            "全部清理，共 {n} 条，保留最近 {AUDIT_KEEP_MIN} 条"
        )),
        operator,
    )?;
    tx.commit()?;
    Ok(n)
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
        let n = clear(&mut conn, "管理员").unwrap();
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
    fn 状态迁移动作有独立中文映射() {
        assert_eq!(action_label("REVERT_ACTIVE"), "回退生效");
        assert_eq!(action_label("PERFORMING"), "转为履行中");
        assert_ne!(action_label("REVERT_ACTIVE"), action_label("ACTIVATE"));
    }
}
