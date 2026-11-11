use crate::error::Result;
use crate::repo;
use ctp_core::dict;
use rusqlite::Connection;

/// 基础字典：会计分类（`ct_category`）与合同类别（`sys_dict_item:CONTRACT_CATEGORY`）。
///
/// 两张字典**分开写库**，是评审 H4 的落地要求：合同类别不得写入 `ct_category`，
/// 否则「工程款」会被当成「前期费用」这类会计科目参与汇总。
pub fn seed_reference(conn: &Connection, operator: &str) -> Result<()> {
    for (i, (code, name)) in dict::ACCOUNTING_CATEGORIES.iter().enumerate() {
        repo::insert_category(conn, code, code, name, i as i64, operator)?;
    }
    for (i, (code, name)) in dict::CONTRACT_CATEGORIES.iter().enumerate() {
        repo::insert_dict_item(conn, "CONTRACT_CATEGORY", code, name, i as i64, operator)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// R-70：启动时重复 seed 不得把（可能已被维护过的）字典名覆盖回默认值。
    #[test]
    fn 重复初始化不覆盖已有字典名称() {
        let mut conn = db::open_in_memory().unwrap();
        db::init_schema(&mut conn).unwrap();
        seed_reference(&conn, "system").unwrap();
        let cats: i64 = conn
            .query_row("SELECT COUNT(*) FROM ct_category", [], |r| r.get(0))
            .unwrap();
        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM sys_dict_item", [], |r| r.get(0))
            .unwrap();
        assert!(cats > 0 && items > 0);
        conn.execute(
            "UPDATE ct_category SET category_name = '自定义会计分类'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE sys_dict_item SET item_name = '自定义合同类别'", [])
            .unwrap();
        seed_reference(&conn, "system").unwrap();
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ct_category WHERE category_name = '自定义会计分类'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, cats, "第二次 seed 不应覆盖改过的会计分类名");
        let kept2: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_dict_item WHERE item_name = '自定义合同类别'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept2, items, "第二次 seed 不应覆盖改过的合同类别名");
    }
}
