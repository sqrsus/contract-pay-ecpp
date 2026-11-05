//! 两套分类字典的**分离定义**（评审 H4）。
//!
//! 文档原稿把 `ct_category` 同时当作「合同类别」（工程款/设备款/材料款/监理费/检测费）
//! 与「合同会计分类」（C01–C99）使用，导致迁移时「工程款」会落到「前期费用」这类科目上。
//! 本实现拆成两张：
//!
//! * **合同类别** —— 业务口径，存 `sys_dict_item(dict_type='CONTRACT_CATEGORY')`，
//!   合同上落在 `ct_contract.contract_category_code`；
//! * **会计分类** —— 财务口径，存 `ct_category(id, category_code)`，
//!   合同上落在 `ct_contract.category_id`（主导分类），清单行落在 `ct_contract_item.category_id`。
//!
//! 二者**不得互相赋值**，报表汇总（R-02 / FR-RPT-02）一律按会计分类口径。

/// 合同类别（业务口径）：(code, 名称)。
pub const CONTRACT_CATEGORIES: &[(&str, &str)] = &[
    ("GC", "工程款"),
    ("SB", "设备款"),
    ("CL", "材料款"),
    ("FW", "监理费"),
    ("JC", "检测费"),
];

/// 会计分类（财务口径）：(code, 名称)。
///
/// 编码区间沿用文档约定的 C01–C99。具体科目名称以建设单位科目表为准，
/// 本处只保证「编码唯一 + 汇总不串科目」，替换时改这一处即可。
pub const ACCOUNTING_CATEGORIES: &[(&str, &str)] = &[
    ("C01", "前期费用"),
    ("C02", "建筑安装工程费"),
    ("C03", "设备及工器具购置费"),
    ("C04", "工程建设其他费"),
    ("C05", "预备费"),
    ("C06", "建设期利息"),
    ("C07", "其他支出"),
];

pub fn contract_category_name(code: &str) -> Option<&'static str> {
    CONTRACT_CATEGORIES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
}

pub fn is_valid_contract_category(code: &str) -> bool {
    contract_category_name(code).is_some()
}

pub fn accounting_category_name(code: &str) -> Option<&'static str> {
    ACCOUNTING_CATEGORIES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
}

pub fn is_valid_accounting_category(code: &str) -> bool {
    accounting_category_name(code).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// H4 回归：两套字典的编码空间不得重叠，否则「工程款」会被当成会计科目。
    #[test]
    fn 合同类别与会计分类编码不重叠() {
        for (bc, _) in CONTRACT_CATEGORIES {
            assert!(
                !ACCOUNTING_CATEGORIES.iter().any(|(ac, _)| ac == bc),
                "编码 {bc} 在两套字典中重复"
            );
        }
    }

    #[test]
    fn 合同类别包含监理费与检测费() {
        assert_eq!(contract_category_name("FW"), Some("监理费"));
        assert_eq!(contract_category_name("JC"), Some("检测费"));
        assert!(is_valid_contract_category("JC"));
    }

    #[test]
    fn 会计分类编码区间为_c01_至_c99() {
        for (code, _) in ACCOUNTING_CATEGORIES {
            assert!(code.starts_with('C'), "会计分类编码应以 C 开头: {code}");
            let n: u32 = code[1..].parse().expect("会计分类编码应为 C+数字");
            assert!((1..=99).contains(&n), "会计分类编码越界: {code}");
        }
    }
}
