//! 操作日志（`sys_audit_log`）的查询与维护。
//!
//! 口径（R-31）：
//! * 时间列存的是 UTC（`now_iso` 的既有约定），**查询与显示一律换算成本地日期/时间**，
//!   否则国内用户会看到「刚刚的操作」停在 8 小时前；
//! * 日期筛选按本地日期闭区间，用 `date(created_at, '+8 hours')` 换算。
//!   函数包列会让 `ix_audit_time` 失效，但日志表本身有 5 万条上限，全扫也在毫秒级；
//! * 关键词同时匹配对象名称、内容与目标 id，够用且不额外建索引。

use crate::error::Result;
use rusqlite::Connection;

pub use crate::db::AUDIT_KEEP_MAX;

/// 一条日志。
#[derive(Debug, Clone)]
pub struct AuditLogRow {
    pub id: String,
    /// 表名式对象类型（`ct_contract` / `pay_payment` …），展示层翻成中文
    pub target_type: String,
    pub target_id: String,
    /// 对象可读名称（R-31 新增列；旧日志为 None，展示层回退到 id）
    pub target_label: Option<String>,
    pub action: String,
    pub detail: Option<String>,
    pub operator_id: Option<String>,
    /// 原始 UTC 时间戳，`YYYY-MM-DDTHH:MM:SSZ`
    pub created_at: String,
    /// 换算后的本地时间，`YYYY-MM-DD HH:MM:SS`，界面直接显示这一列
    pub created_local: String,
}

/// 日志**列表筛选**条件。`from` / `to` 是本地日期闭区间，空串表示不限。
///
/// R-70 起仅用于查询与显示，不再作为清理范围参数。
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub from: String,
    pub to: String,
    /// 操作类型（`action` 原值）；None 表示全部
    pub action: Option<String>,
    /// 关键词：同时匹配对象名称、操作内容、目标 id
    pub keyword: String,
}

impl LogFilter {
    /// 四个条件写成「参数为空即放行」，避免动态拼 SQL 带来的参数错位。
    ///
    /// 关键词按**字面子串**匹配：`ESCAPE '\'` + `db::like_escape` 把 `%` / `_` /
    /// `\` 转义掉。否则输入一个 `%` 就命中全表，筛选结果会失真。
    fn where_clause() -> &'static str {
        "WHERE (?1 = '' OR date(created_at, '+8 hours') >= ?1) \
           AND (?2 = '' OR date(created_at, '+8 hours') <= ?2) \
           AND (?3 = '' OR action = ?3) \
           AND (?4 = '' OR COALESCE(target_label, '') LIKE '%' || ?4 || '%' ESCAPE '\\' \
                         OR COALESCE(detail, '') LIKE '%' || ?4 || '%' ESCAPE '\\' \
                         OR target_id LIKE '%' || ?4 || '%' ESCAPE '\\')"
    }

    fn params(&self) -> [String; 4] {
        [
            self.from.trim().to_string(),
            self.to.trim().to_string(),
            self.action.clone().unwrap_or_default(),
            crate::db::like_escape(self.keyword.trim()),
        ]
    }
}

const SELECT_COLS: &str = "id, target_type, target_id, target_label, action, detail, operator_id, \
     created_at, datetime(created_at, '+8 hours') AS created_local";

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditLogRow> {
    Ok(AuditLogRow {
        id: r.get(0)?,
        target_type: r.get(1)?,
        target_id: r.get(2)?,
        target_label: r.get(3)?,
        action: r.get(4)?,
        detail: r.get(5)?,
        operator_id: r.get(6)?,
        created_at: r.get(7)?,
        created_local: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
    })
}

/// 按条件取日志，新的在前。`limit` 只影响显示条数，不影响合计。
pub fn list(conn: &Connection, f: &LogFilter, limit: i64) -> Result<Vec<AuditLogRow>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM sys_audit_log {} ORDER BY created_at DESC, id DESC LIMIT ?5",
        LogFilter::where_clause()
    );
    let p = f.params();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            rusqlite::params![p[0], p[1], p[2], p[3], limit.max(1)],
            row_of,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 同一条件下有多少条（用于「共 N 条」与清理确认）。
pub fn count(conn: &Connection, f: &LogFilter) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM sys_audit_log {}",
        LogFilter::where_clause()
    );
    let p = f.params();
    let n: i64 = conn.query_row(&sql, rusqlite::params![p[0], p[1], p[2], p[3]], |r| {
        r.get(0)
    })?;
    Ok(n)
}

/// 全表条数（不受筛选影响），页面用来显示容量占用。
pub fn total(conn: &Connection) -> Result<i64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))?;
    Ok(n)
}

/// 出现过的操作类型，按出现次数从多到少。供界面做筛选项，不用硬编码列表。
pub fn actions(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT action FROM sys_audit_log GROUP BY action ORDER BY COUNT(*) DESC")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 一次性清空日志时至少保留的最近条数（R-68 / C14-A）。
///
/// 全删不可恢复，误触一次就把审计链清零；清空后留下最新的 N 条，
/// 并额外写一条 `CLEAR` 留痕（见 `ctp_app::logs::clear`）。
pub const AUDIT_KEEP_MIN: usize = 200;

/// 一次性清空日志：删除除最近 [`AUDIT_KEEP_MIN`] 条以外的全部记录，返回删除条数。
///
/// 按日志 id 序保留最近 N 条——id 形如 LOG-{毫秒}-{序号}，固定宽度使字典序即时间序，
/// 与 `db::insert_audit_log` 的裁剪口径一致。
/// R-70 起不再提供「按时间段 / 按条件」清理入口，删除只走这一条路径；
/// 因为只有全清一种操作，保底无条件生效，不再需要按「删完还剩多少」判定。
pub fn clear_all(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM sys_audit_log \
         WHERE id NOT IN (SELECT id FROM sys_audit_log ORDER BY id DESC LIMIT ?1)",
        rusqlite::params![AUDIT_KEEP_MIN as i64],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use ctp_core::date::now_iso;

    fn fixture() -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        db::init_schema(&mut conn).unwrap();
        conn
    }

    fn add(conn: &Connection, label: &str, action: &str, detail: &str) {
        db::insert_audit_log(
            conn,
            "ct_contract",
            "CT-1",
            Some(label),
            action,
            Some(detail),
            "管理员",
        )
        .unwrap();
    }

    #[test]
    fn 按类型与关键词过滤() {
        let conn = fixture();
        add(&conn, "HT-001 道路改造", "CREATE", "新建合同");
        add(&conn, "HT-002 桥梁维修", "REGISTER", "登记拨款 10 万");
        let f = LogFilter {
            action: Some("CREATE".into()),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f).unwrap(), 1);
        let f = LogFilter {
            keyword: "桥梁".into(),
            ..Default::default()
        };
        let rows = list(&conn, &f, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target_label.as_deref(), Some("HT-002 桥梁维修"));
    }

    #[test]
    fn 日期筛选按本地日期换算() {
        let conn = fixture();
        add(&conn, "HT-001", "CREATE", "新建合同");
        // 列表筛选的日期区间（R-70 保留功能）：now_iso 是 UTC，
        // 本地日期取它的 +8 版本，用同一天筛必然命中
        let utc = now_iso();
        let local_date: String = conn
            .query_row("SELECT date(?1, '+8 hours')", rusqlite::params![utc], |r| {
                r.get(0)
            })
            .unwrap();
        let f = LogFilter {
            from: local_date.clone(),
            to: local_date.clone(),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f).unwrap(), 1, "本地日期当天应命中");
        let f = LogFilter {
            from: "2000-01-01".into(),
            to: "2000-01-02".into(),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f).unwrap(), 0);
    }

    /// 一次性清空必须保底保留最近 200 条，且重复清空不会继续删。
    #[test]
    fn 清空日志保底保留最近两百条() {
        let conn = fixture();
        for i in 0..205 {
            add(&conn, "HT-001", "CREATE", &format!("第 {i} 条"));
        }
        assert_eq!(clear_all(&conn).unwrap(), 5);
        assert_eq!(total(&conn).unwrap(), 200);
        // 再来一次不会继续删
        assert_eq!(clear_all(&conn).unwrap(), 0);
        assert_eq!(total(&conn).unwrap(), 200);
        // 留下的是最新的那些（detail 带行号，按 id 序倒排取第一条）
        let rows = list(&conn, &LogFilter::default(), 1).unwrap();
        assert_eq!(rows[0].detail.as_deref(), Some("第 204 条"));
    }

    /// 关键词是字面子串：`%` / `_` 不再充当 LIKE 通配，避免误清全表。
    #[test]
    fn 关键词按字面匹配不把通配符当模式() {
        let conn = fixture();
        add(&conn, "HT-001", "CREATE", "明细 100%");
        add(&conn, "HT-002", "CREATE", "明细正常");
        let f = LogFilter {
            keyword: "%".into(),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f).unwrap(), 1, "只有含字面 % 的那条命中");
        let f2 = LogFilter {
            keyword: "明细".into(),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f2).unwrap(), 2);
        let f3 = LogFilter {
            keyword: "\\".into(),
            ..Default::default()
        };
        assert_eq!(count(&conn, &f3).unwrap(), 0, "反斜杠也必须按字面处理");
    }
}
