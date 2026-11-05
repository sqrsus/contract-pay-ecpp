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

    /// 从「元」字符串解析，如 `1234567.89`。宽容解析（便于从 Excel 粘贴）：
    /// 空串 → 0；自动去千分位与空白；小数超过两位按第三位四舍五入到分。
    pub fn parse_yuan(s: &str) -> Result<Self> {
        let cleaned = s.trim().replace(',', "").replace(' ', "");
        if cleaned.is_empty() {
            return Ok(Money::ZERO);
        }
        let neg = cleaned.starts_with('-');
        let body = cleaned.trim_start_matches(['-', '+']);
        let (int_part, frac_part) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        let int_v: i64 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| DomainError::MoneyFormat(cleaned.clone()))?
        };
        let mut digits: Vec<u8> = frac_part.bytes().filter(|b| b.is_ascii_digit()).collect();
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
        let s = if a % 100 == 0 {
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

impl std::ops::Add for Money {
    type Output = Money;
    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Money {
    type Output = Money;
    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}

impl From<i64> for Money {
    fn from(v: i64) -> Self {
        Money(v)
    }
}

#[cfg(test)]
mod tests {
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
}
