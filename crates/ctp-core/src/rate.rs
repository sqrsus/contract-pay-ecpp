use crate::money::Money;

/// 比率，单位：万分之一（1 bp = 0.01%）。
///
/// 全系统比率的**唯一实现**（文档 5.5 视图使用约束第 5 条 / 6.2.1）：
/// 视图 `pay_rate_bp` 的 SQL 表达式必须与本类型逐位一致，见
/// `ctp-store::schema::rate_bp_expr`，并由 `selfcheck` 断言差异为 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Rate(i32);

impl Rate {
    pub const ZERO: Rate = Rate(0);
    pub const FULL: Rate = Rate(10_000);

    pub fn from_bp(bp: i32) -> Self {
        Rate(bp)
    }

    pub fn bp(self) -> i32 {
        self.0
    }

    pub fn as_percent(self) -> f64 {
        self.0 as f64 / 100.0
    }

    /// 展示用两位小数百分比，如 `33.33%`。
    pub fn percent_string(self) -> String {
        format!("{:.2}%", self.as_percent())
    }

    /// 分子/分母，四舍五入到 bp；分母 <= 0 时返回 0（不除零、不 panic）。
    ///
    /// 舍入规则：分子 >= 0 取 `(n*2 + d) / (d*2)`，否则 `(n*2 - d) / (d*2)`，均向零截断。
    /// 展开后即 `(numer*20000 ± denom) / (denom*2)`——**`*2` 必须在分母上**，
    /// 文档 5.5 V1 曾写成 `... / base * 2`（评审 H1），版本库内已统一为分母乘法。
    pub fn ratio(numer: Money, denom: Money) -> Rate {
        if denom.cent() <= 0 {
            return Rate::ZERO;
        }
        let n = (numer.cent() as i128) * 10_000;
        let d = denom.cent() as i128;
        let bp = if n >= 0 {
            (n * 2 + d) / (d * 2)
        } else {
            (n * 2 - d) / (d * 2)
        };
        Rate(i32::try_from(bp).unwrap_or(if bp > 0 { i32::MAX } else { i32::MIN }))
    }
}

impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.percent_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 评审 H1 的回归断言：文档 V1 原式 `(paid*20000+base)/base*2` 得 13334 bp，
    /// 正确值 3333 bp。此处锁死正确值，任何把它改回「除后乘」的改动都会失败。
    #[test]
    fn 比率不得在除法之后再乘二() {
        let paid = Money::from_cent(100);
        let base = Money::from_cent(300);
        let r = Rate::ratio(paid, base);
        assert_eq!(r.bp(), 3_333, "100/300 应为 3333 bp（33.33%），实得 {}", r.bp());
        assert_ne!(r.bp(), 13_334, "这是除后乘 2 的错误结果");
    }

    #[test]
    fn 比率边界与负数() {
        assert_eq!(Rate::ratio(Money::ZERO, Money::from_cent(300)).bp(), 0);
        assert_eq!(
            Rate::ratio(Money::from_cent(300), Money::from_cent(300)).bp(),
            10_000
        );
        // 分母 <= 0：返回 0，不 panic
        assert_eq!(Rate::ratio(Money::from_cent(1), Money::ZERO).bp(), 0);
        assert_eq!(
            Rate::ratio(Money::from_cent(1), Money::from_cent(-5)).bp(),
            0
        );
        // 负数向零截断：-100/300 = -3333 bp
        assert_eq!(
            Rate::ratio(Money::from_cent(-100), Money::from_cent(300)).bp(),
            -3_333
        );
        // 半值四舍五入：150/300 = 5000 bp
        assert_eq!(
            Rate::ratio(Money::from_cent(150), Money::from_cent(300)).bp(),
            5_000
        );
        // 1/3 -> 3333.33 -> 3333（不足半分不进位）
        assert_eq!(
            Rate::ratio(Money::from_cent(1), Money::from_cent(3)).bp(),
            3_333
        );
    }
}
