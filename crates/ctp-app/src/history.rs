use ctp_store::db;
use ctp_store::{Result, StoreError};
use rusqlite::Connection;
use serde_json::{json, Value};

/// 历史键：合同金额编辑（BR-22）。
pub const KEY_CONTRACT_AMOUNT: &str = "contract_amount_history";
/// 历史键：审计价修改 / 清空（BR-16）。
pub const KEY_AUDIT_PRICE: &str = "audit_price_history";

/// 保留条数上限（文档 4.2.6 / 4.2.7：最近 50 条）。
pub const HISTORY_LIMIT: usize = 50;

/// 追加一条历史。`sys_meta.meta_value` 存 JSON 数组，超出上限丢弃最旧的一条。
pub fn append(conn: &Connection, key: &str, entry: Value, operator: &str) -> Result<()> {
    let mut arr = load(conn, key)?;
    arr.push(entry);
    let len = arr.len();
    if len > HISTORY_LIMIT {
        arr.drain(0..len - HISTORY_LIMIT);
    }
    let s = serde_json::to_string(&arr)
        .map_err(|e| StoreError::Inconsistent(format!("历史序列化失败：{e}")))?;
    db::set_meta(conn, key, &s, operator)
}

pub fn load(conn: &Connection, key: &str) -> Result<Vec<Value>> {
    let raw = db::get_meta(conn, key)?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default())
}

/// 构造一条金额变更记录（元与分都留档，便于人工核对）。
pub fn amount_entry(
    at: &str,
    old_cent: i64,
    new_cent: i64,
    reason: &str,
    operator: &str,
) -> Value {
    json!({
        "at": at,
        "old_cent": old_cent,
        "new_cent": new_cent,
        "old_yuan": ctp_core::money::Money::from_cent(old_cent).format_yuan(),
        "new_yuan": ctp_core::money::Money::from_cent(new_cent).format_yuan(),
        "reason": reason,
        "operator": operator,
    })
}

/// 构造一条审计价变更记录；`new_cent` 为 None 表示清空（回到未审定态）。
pub fn audit_entry(
    at: &str,
    old_cent: Option<i64>,
    new_cent: Option<i64>,
    reason: &str,
    operator: &str,
) -> Value {
    json!({
        "at": at,
        "old_cent": old_cent,
        "new_cent": new_cent,
        "cleared": new_cent.is_none(),
        "reason": reason,
        "operator": operator,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 历史只保留最近五十条() {
        let mut conn = ctp_store::db::open_in_memory().unwrap();
        ctp_store::db::migrate(&mut conn).unwrap();
        for i in 0..60 {
            append(
                &conn,
                KEY_CONTRACT_AMOUNT,
                amount_entry("2026-01-01", 0, i, "test", "u"),
                "u",
            )
            .unwrap();
        }
        let arr = load(&conn, KEY_CONTRACT_AMOUNT).unwrap();
        assert_eq!(arr.len(), HISTORY_LIMIT);
        assert_eq!(arr[0]["new_cent"], 10);
        assert_eq!(arr[49]["new_cent"], 59);
    }
}
