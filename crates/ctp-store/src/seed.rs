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
