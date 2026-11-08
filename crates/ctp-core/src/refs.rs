//! 编号前缀规范（评审 M6）。
//!
//! 原文档中「修订记录 R-xx」「风险 R-xx」「报表 R-xx」三套体系共用 `R-` 前缀，
//! 出现了「R-05、R-06、R-08 随 R-05 移出」这类一名两义的句子，维护时极易串号。
//! 本模块把三类编号的**唯一前缀**固定下来，新增代码只允许用下列前缀：
//!
//! * 修订记录 —— `RV-`
//! * 风险 —— `RK-`
//! * 报表 —— `RPT-`
//!
//! 旧 `R-` 前缀被 `parse` 显式拒绝，避免新代码继续沿用歧义编号。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Revision,
    Risk,
    Report,
}

impl RefKind {
    pub fn prefix(self) -> &'static str {
        match self {
            RefKind::Revision => "RV-",
            RefKind::Risk => "RK-",
            RefKind::Report => "RPT-",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RefKind::Revision => "修订",
            RefKind::Risk => "风险",
            RefKind::Report => "报表",
        }
    }

    /// 解析编号：必须形如 `RV-01` / `RK-02` / `RPT-11`（前缀大写 + 至少一位数字）。
    /// `R-11` 这类旧写法返回 None——它无法判定属于哪一套体系。
    pub fn parse(code: &str) -> Option<(RefKind, u32)> {
        let (kind, rest) = if let Some(r) = code.strip_prefix("RPT-") {
            (RefKind::Report, r)
        } else if let Some(r) = code.strip_prefix("RV-") {
            (RefKind::Revision, r)
        } else {
            let r = code.strip_prefix("RK-")?;
            (RefKind::Risk, r)
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((kind, rest.parse().ok()?))
    }

    pub fn format(self, n: u32) -> String {
        format!("{}{:02}", self.prefix(), n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 三类编号互不串号() {
        assert_eq!(RefKind::parse("RV-05"), Some((RefKind::Revision, 5)));
        assert_eq!(RefKind::parse("RK-05"), Some((RefKind::Risk, 5)));
        assert_eq!(RefKind::parse("RPT-05"), Some((RefKind::Report, 5)));
        assert_eq!(RefKind::Revision.format(5), "RV-05");
        assert_eq!(RefKind::Report.format(11), "RPT-11");
    }

    #[test]
    fn 旧前缀与非法编号被拒绝() {
        assert_eq!(RefKind::parse("R-05"), None);
        assert_eq!(RefKind::parse("RV-"), None);
        assert_eq!(RefKind::parse("RV-X"), None);
        assert_eq!(RefKind::parse("rv-05"), None);
    }
}
