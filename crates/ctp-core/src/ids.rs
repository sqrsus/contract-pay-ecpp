use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 生成主键：`{前缀}-{12 位毫秒时间戳}-{4 位序号}`。
///
/// 时间戳在前保证大体有序，序号避免同毫秒碰撞。业务编号（合同号、拨款单号）
/// 另有生成规则（4.2.4），**不要**用本函数代替。
pub fn new_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{millis:012X}-{:04X}", n & 0xFFFF)
}

/// 合同编号（当前实现口径）：`HT-{年度}-{三位流水}`，如 `HT-2026-007`（4.2.4）。
/// 目标态 `HT-{项目编码}-{年度}-{四位流水}` 属二期，此处保留单一口径便于切换。
pub fn contract_no(year: &str, seq: i64) -> String {
    format!("HT-{year}-{seq:03}")
}

/// 拨款单号：`BK-{年度}-{四位流水}`。
pub fn payment_no(year: &str, seq: i64) -> String {
    format!("BK-{year}-{seq:04}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 主键唯一且带前缀() {
        let a = new_id("CT");
        let b = new_id("CT");
        assert_ne!(a, b);
        assert!(a.starts_with("CT-"));
    }

    #[test]
    fn 业务编号格式() {
        assert_eq!(contract_no("2026", 7), "HT-2026-007");
        assert_eq!(payment_no("2026", 12), "BK-2026-0012");
    }
}
