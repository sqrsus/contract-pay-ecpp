//! 人民币金额 → 中文大写（财务票据用）。
//!
//! 口径：**零壹贰叁肆伍陆柒捌玖 / 拾佰仟万亿 / 圆角分 / 整**，不带「人民币」前缀。
//! 输入单位恒为「分」（`Money::cent()`），本模块不参与任何业务金额运算，只做展示转换。
//!
//! 例：
//! * `0` → `零圆整`
//! * `100_005` 分（1000.05 元）→ `壹仟圆零伍分`
//! * `123_456_789` 分（1,234,567.89 元）→ `壹佰贰拾叁万肆仟伍佰陆拾柒圆捌角玖分`

const DIGITS: [&str; 10] = ["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];
const SMALL_UNITS: [&str; 4] = ["", "拾", "佰", "仟"];
// u64 最多 5 个四位分组（兆、京），给足 6 个避免越界
const BIG_UNITS: [&str; 6] = ["", "万", "亿", "兆", "京", "垓"];

/// 整数（元）部分转大写。`n == 0` 返回空串，由调用方补「零圆」。
fn integer_to_cn(n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    // groups[0] 是个/十/百/千位，groups[1] 是万位段，依次类推
    let mut groups: Vec<u16> = Vec::new();
    let mut rest = n;
    while rest > 0 {
        groups.push((rest % 10_000) as u16);
        rest /= 10_000;
    }

    let mut out = String::new();
    for gi in (0..groups.len()).rev() {
        let g = groups[gi] as usize;
        if g == 0 {
            // 中间整段为零：只要后面还有非零段，补一个「零」（连续零只留一个）
            let lower_has_value = groups[..gi].iter().any(|&v| v != 0);
            if lower_has_value && !out.is_empty() && !out.ends_with('零') {
                out.push('零');
            }
            continue;
        }
        // 本段不足四位（如 0001）且前面已有高位段，需补「零」：壹万零壹
        if g < 1_000 && !out.is_empty() && !out.ends_with('零') {
            out.push('零');
        }
        let ds = [g / 1_000, (g / 100) % 10, (g / 10) % 10, g % 10];
        // 段内单独渲染：前导零不能触发补零，尾部零也不该输出（1000 → 壹仟）
        let mut gstr = String::new();
        let mut zero_pending = false;
        for (i, d) in ds.iter().copied().enumerate() {
            if d == 0 {
                if !gstr.is_empty() {
                    zero_pending = true;
                }
            } else {
                if zero_pending {
                    gstr.push('零');
                    zero_pending = false;
                }
                gstr.push_str(DIGITS[d]);
                gstr.push_str(SMALL_UNITS[3 - i]);
            }
        }
        out.push_str(&gstr);
        if let Some(unit) = BIG_UNITS.get(gi) {
            out.push_str(unit);
        }
    }
    out
}

/// 金额（分）转人民币中文大写。
pub fn rmb_uppercase(cent: i64) -> String {
    if cent == 0 {
        return "零圆整".to_string();
    }
    let neg = cent < 0;
    let a = cent.unsigned_abs();
    let yuan = a / 100;
    let jiao = (a / 10) % 10;
    let fen = a % 10;

    let int_cn = integer_to_cn(yuan);
    let mut s = String::new();
    if neg {
        s.push('负');
    }
    if int_cn.is_empty() {
        s.push_str("零圆");
    } else {
        s.push_str(&int_cn);
        s.push('圆');
    }

    if jiao == 0 && fen == 0 {
        s.push('整');
    } else {
        if jiao > 0 {
            s.push_str(DIGITS[jiao as usize]);
            s.push('角');
        } else {
            // 有分无角：壹圆零伍分 / 零圆零伍分
            s.push('零');
        }
        if fen > 0 {
            s.push_str(DIGITS[fen as usize]);
            s.push('分');
        } else {
            s.push('整');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 零与分角() {
        assert_eq!(rmb_uppercase(0), "零圆整");
        assert_eq!(rmb_uppercase(1), "零圆零壹分");
        assert_eq!(rmb_uppercase(5), "零圆零伍分");
        assert_eq!(rmb_uppercase(10), "零圆壹角整");
        assert_eq!(rmb_uppercase(15), "零圆壹角伍分");
    }

    #[test]
    fn 整元与中间零() {
        assert_eq!(rmb_uppercase(100_000), "壹仟圆整");
        assert_eq!(rmb_uppercase(1_000_105), "壹万零壹圆零伍分");
        assert_eq!(rmb_uppercase(1_000_005), "壹万圆零伍分");
        assert_eq!(rmb_uppercase(100_005), "壹仟圆零伍分");
        assert_eq!(rmb_uppercase(1_001_00), "壹仟零壹圆整");
    }

    #[test]
    fn 四则常见金额() {
        assert_eq!(
            rmb_uppercase(123_456_789),
            "壹佰贰拾叁万肆仟伍佰陆拾柒圆捌角玖分"
        );
        assert_eq!(rmb_uppercase(1_250_000_00), "壹佰贰拾伍万圆整");
        assert_eq!(rmb_uppercase(100_000_000_00), "壹亿圆整");
        assert_eq!(rmb_uppercase(100_010_000_00), "壹亿零壹万圆整");
    }

    #[test]
    fn 负数加前缀() {
        assert_eq!(rmb_uppercase(-12_345), "负壹佰贰拾叁圆肆角伍分");
    }
}
