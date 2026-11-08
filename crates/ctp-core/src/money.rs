use crate::error::{DomainError, Result};
use std::fmt;

/// 金额，单位：分。禁止直接用 i64 参与业务运算（文档 6.2.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Money(i64);

impl Money {
    pub const ZERO: Money = Money(0);

    pub fn from_cent(c: i64) -> Self {
        Money(c)
    }

    pub fn cent(self) -> i64 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn abs(self) -> Money {
        Money(self.0.saturating_abs())
    }

    /// 单笔金额上限：100 亿元（= 10^12 分）。录入与导入都在此拦截，
    /// 让后续汇总求和在 i64 下没有溢出空间（R-39 审查整改）。
    pub const MAX_SINGLE_CENT: i64 = 1_000_000_000_000;

    /// 从「元」字符串解析，如 `1234567.89`。宽容之处仅限（便于从 Excel 粘贴）：
    /// 空串 → 0；自动去千分位与空白；小数超过两位按第三位四舍五入到分。
    ///
    /// 其余一律**严格拒绝**，不再把非法字符过滤成另一个金额：
    /// - 非法小数字符（`1.2x`、`1.2-3`）；
    /// - 重复小数点（`12.3.4`）；
    /// - 异常符号（`+-12`、`--12`、`1+2`、只有符号没有数字）。
    ///
    /// 单笔超过 [`Money::MAX_SINGLE_CENT`] 也明确报溢出，不静默接受。
    pub fn parse_yuan(s: &str) -> Result<Self> {
        let cleaned = s.trim().replace([',', ' '], "");
        if cleaned.is_empty() {
            return Ok(Money::ZERO);
        }
        // 符号：只允许一个可选的前导 +/-
        let (neg, body) = if let Some(rest) = cleaned.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = cleaned.strip_prefix('+') {
            (false, rest)
        } else {
            (false, cleaned.as_str())
        };
        // body 里不得再出现任何符号，且至少要有一个数字
        if body.is_empty() || body.contains(['-', '+']) || !body.bytes().any(|b| b.is_ascii_digit())
        {
            return Err(DomainError::MoneyFormat(cleaned.clone()));
        }
        // 小数点最多一个；整数段、小数段都必须是纯 ASCII 数字（允许某一段为空）
        let mut parts = body.split('.');
        let int_part = parts.next().unwrap_or("");
        let frac_part = parts.next();
        if parts.next().is_some() {
            return Err(DomainError::MoneyFormat(cleaned.clone())); // 重复小数点
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DomainError::MoneyFormat(cleaned.clone()));
        }
        let frac_part = frac_part.unwrap_or("");
        if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DomainError::MoneyFormat(cleaned.clone())); // 非法小数字符
        }
        let int_v: i64 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| DomainError::MoneyFormat(cleaned.clone()))?
        };
        let mut digits: Vec<u8> = frac_part.bytes().collect();
        let round_up = digits.len() > 2 && digits[2] >= b'5';
        digits.truncate(2);
        while digits.len() < 2 {
            digits.push(b'0');
        }
        let frac_v: i64 = std::str::from_utf8(&digits)
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| DomainError::MoneyFormat(cleaned.clone()))?;
        let mut cent = int_v
            .checked_mul(100)
            .and_then(|v| v.checked_add(frac_v))
            .ok_or(DomainError::MoneyOverflow)?;
        // 进位必须在「整数×100 + 小数」合成之后做，否则 0.999 会被算成 99 分（应 100 分）
        if round_up {
            cent = cent.checked_add(1).ok_or(DomainError::MoneyOverflow)?;
        }
        if cent > Money::MAX_SINGLE_CENT {
            return Err(DomainError::MoneyOverflow);
        }
        Ok(Money(if neg { -cent } else { cent }))
    }

    /// 解析失败的场景按 0 处理，只用于导入容错，业务录入请用 `parse_yuan`。
    pub fn parse_yuan_lenient(s: &str) -> Money {
        Money::parse_yuan(s).unwrap_or(Money::ZERO)
    }

    pub fn checked_add(self, o: Money) -> Result<Money> {
        self.0
            .checked_add(o.0)
            .map(Money)
            .ok_or(DomainError::MoneyOverflow)
    }

    pub fn checked_sub(self, o: Money) -> Result<Money> {
        self.0
            .checked_sub(o.0)
            .map(Money)
            .ok_or(DomainError::MoneyOverflow)
    }

    /// 纯展示派生值用的饱和加减：越界时钳到 `i64` 边界，不 panic、不回绕成负数。
    /// 这些派生函数签名返回 `Money` 而非 `Result<Money>`（改动会波及大量调用方），
    /// 因此用饱和代替裸 `+` / `-`；真正的录入/写入路径一律走上面的 checked 版本。
    pub fn saturating_add(self, o: Money) -> Money {
        Money(self.0.saturating_add(o.0))
    }

    pub fn saturating_sub(self, o: Money) -> Money {
        Money(self.0.saturating_sub(o.0))
    }

    /// 汇总求和（报表 / KPI 用）：逐项饱和累加，越界时钳到 `i64` 边界，
    /// 既不在 debug 下 panic，也不在 release 下静默回绕成负数。
    ///
    /// 这些构造函数返回 `Self` 而非 `Result`，无法把溢出错误向上抛；
    /// 因此配合 [`Money::MAX_SINGLE_CENT`] 单笔上限构成双保险，
    /// 而写入与额度校验路径仍一律使用 `checked_add` / `checked_sub` 返回错误。
    pub fn sum_saturating(it: impl IntoIterator<Item = i64>) -> i64 {
        it.into_iter().fold(0_i64, |acc, v| acc.saturating_add(v))
    }

    /// 按比率（万分比）计取，四舍五入到分。
    pub fn mul_rate(self, rate_bp: i32) -> Result<Money> {
        let v = (self.0 as i128) * (rate_bp as i128);
        let rounded = if v >= 0 {
            (v + 5_000) / 10_000
        } else {
            (v - 5_000) / 10_000
        };
        i64::try_from(rounded)
            .map(Money)
            .map_err(|_| DomainError::MoneyOverflow)
    }

    /// 打印 / 导出用：固定两位小数的「元」字符串，无千分位，如 `1250000.00`。
    ///
    /// 模板里的合并域带 `\# "0.00"` 数字开关，填充成纯文本时须保持同样的形态；
    /// 与 `format_yuan()` 的千分位展示口径不同，不要混用。
    pub fn fixed_yuan(self) -> String {
        let a = self.0.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        format!("{sign}{}.{:02}", a / 100, a % 100)
    }

    /// 财务票据的中文大写（零壹贰…圆角分整），见 `amount_cn`。
    pub fn rmb_uppercase(self) -> String {
        crate::amount_cn::rmb_uppercase(self.0)
    }

    /// 展示用：整数不带小数位（1,280,000），有角分时保留两位（12,345.67）。
    /// 负数按财务惯例用括号包裹：`(1,234.56)`。
    pub fn format_yuan(self) -> String {
        let neg = self.0 < 0;
        let a = self.0.unsigned_abs();
        let s = if a.is_multiple_of(100) {
            format!("{}", a / 100)
        } else {
            format!("{}.{:02}", a / 100, a % 100)
        };
        let s = add_thousand_sep(&s);
        if neg {
            format!("({s})")
        } else {
            s
        }
    }
}

fn add_thousand_sep(s: &str) -> String {
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let bytes = int_part.as_bytes();
    let mut out = String::with_capacity(int_part.len() + int_part.len() / 3 + 4);
    for (idx, b) in bytes.iter().enumerate() {
        let remaining = bytes.len() - idx;
        if idx > 0 && remaining % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    match frac {
        Some(f) => format!("{out}.{f}"),
        None => out,
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format_yuan())
    }
}

// R-39：不再实现 `Add` / `Sub`。裸 `+` / `-` 在 release 下静默回绕、debug 下 panic，
// 财务金额必须显式选择 `checked_*`（写入/校验，返回错误）或 `saturating_*`（纯展示派生）。

impl From<i64> for Money {
    fn from(v: i64) -> Self {
        Money(v)
    }
}

#[cfg(test)]
mod tests {
    // 金额字面量刻意按「元_分」分组（如 1_250_000_00 = 1,250,000.00 元），便于人工核对。
    #![allow(clippy::inconsistent_digit_grouping)]
    use super::*;

    #[test]
    fn 人民币解析与格式化口径一致() {
        assert_eq!(Money::parse_yuan("").unwrap(), Money::ZERO);
        assert_eq!(Money::parse_yuan("  1,234.56 ").unwrap().cent(), 123_456);
        assert_eq!(Money::parse_yuan("0.999").unwrap().cent(), 100);
        assert_eq!(Money::parse_yuan("12.345").unwrap().cent(), 1_235);
        assert_eq!(Money::parse_yuan("-12.345").unwrap().cent(), -1_235);
        assert_eq!(Money::from_cent(128_000_000).format_yuan(), "1,280,000");
        assert_eq!(Money::from_cent(1_234_567).format_yuan(), "12,345.67");
        assert_eq!(Money::from_cent(-123_456).format_yuan(), "(1,234.56)");
    }

    #[test]
    fn 打印用固定两位与中文大写() {
        assert_eq!(Money::from_cent(1_250_000_00).fixed_yuan(), "1250000.00");
        assert_eq!(Money::from_cent(5).fixed_yuan(), "0.05");
        assert_eq!(Money::ZERO.fixed_yuan(), "0.00");
        assert_eq!(
            Money::from_cent(1_250_000_00).rmb_uppercase(),
            "壹佰贰拾伍万圆整"
        );
    }

    #[test]
    fn 按比率计取四舍五入到分() {
        // 3% 的 100 元 = 3 元整
        assert_eq!(Money::from_cent(10_000).mul_rate(300).unwrap().cent(), 300);
        // 1 分 × 5% = 0.05 分 → 0 分
        assert_eq!(Money::from_cent(1).mul_rate(500).unwrap().cent(), 0);
        // 30 分 × 5% = 1.5 分 → 2 分（四舍五入）
        assert_eq!(Money::from_cent(30).mul_rate(500).unwrap().cent(), 2);
    }

    /// R-39：非法字符不能再被「洗」成另一个金额。
    #[test]
    fn 非法金额字符一律拒绝而非清洗() {
        for bad in [
            "12.3.4", // 重复小数点（旧实现会静默变成 12.34）
            "1.2x",   // 非法小数字符（旧实现会变成 1.20）
            "1.2-3",  // 小数段出现符号
            "+-12", "--12", "1+2", "-", "+", // 异常符号 / 只有符号
            "abc", "1,2x3", // 非数字
        ] {
            assert!(
                Money::parse_yuan(bad).is_err(),
                "「{bad}」应被拒绝，实得 {:?}",
                Money::parse_yuan(bad)
            );
        }
        // 既有宽容口径保持：千分位、前后空白、第三位四舍五入、允许 ".5" / "1."
        assert_eq!(Money::parse_yuan(" 1,234.56 ").unwrap().cent(), 123_456);
        assert_eq!(Money::parse_yuan("0.999").unwrap().cent(), 100);
        assert_eq!(Money::parse_yuan(".5").unwrap().cent(), 50);
        assert_eq!(Money::parse_yuan("1.").unwrap().cent(), 100);
    }

    /// R-39：单笔上限与 i64 边界。
    #[test]
    fn 单笔上限与溢出边界() {
        assert_eq!(
            Money::parse_yuan("10000000000").unwrap().cent(),
            Money::MAX_SINGLE_CENT,
            "100 亿元整正好等于上限，应放行"
        );
        assert!(
            Money::parse_yuan("10000000000.01").is_err(),
            "超出 1 分即拒绝"
        );
        assert!(
            Money::parse_yuan("-10000000000.01").is_err(),
            "负向同样拒绝"
        );
        assert!(
            Money::parse_yuan(&"9".repeat(30)).is_err(),
            "远超 i64 应报错而非回绕"
        );
        assert!(Money::from_cent(i64::MAX)
            .checked_add(Money::from_cent(1))
            .is_err());
        assert!(Money::from_cent(i64::MIN)
            .checked_sub(Money::from_cent(1))
            .is_err());
    }

    #[test]
    fn 饱和派生与饱和汇总不回绕() {
        assert_eq!(
            Money::from_cent(i64::MAX)
                .saturating_add(Money::from_cent(1))
                .cent(),
            i64::MAX
        );
        assert_eq!(
            Money::from_cent(i64::MIN)
                .saturating_sub(Money::from_cent(1))
                .cent(),
            i64::MIN
        );
        assert_eq!(
            Money::sum_saturating([i64::MAX, 1, 1]),
            i64::MAX,
            "越界应钳到 i64 最大值，而不是变成负数"
        );
        assert_eq!(Money::sum_saturating([1, 2, 3]), 6);
        assert_eq!(Money::sum_saturating(Vec::<i64>::new()), 0);
    }
}
