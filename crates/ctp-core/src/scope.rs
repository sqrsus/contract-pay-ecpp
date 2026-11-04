//! 本期范围与优先级台账（评审 M5 的可执行版本）。
//!
//! 评审指出：`FR-CT-05` / `FR-SYS-05` / `FR-SYS-07` 在 2.4 标为 P0，却在实现状态处
//! 标「未实现」并列入「不据此拒收」白名单；同一待办 `E-16` 在 11.8.1 是 P0、在附录 E 是 P1。
//! 结果使 11.8「范围内 P0 100% 通过」的判据失效。
//!
//! 本模块把口径固化为一条可执行规则：
//! **P0 必须属于本期范围（`Scope::InPhase`）；凡列入白名单（延期）的条目一律不得为 P0。**
//! `audit_p0_consistency()` 违反即返回违例，`selfcheck` 命中时以非零退出码结束。

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    P0,
    P1,
    P2,
}

impl Priority {
    pub fn code(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
        }
    }
}

/// 本期是否交付。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 本期范围内，必须交付
    InPhase,
    /// 延期（原白名单项），不作为拒收依据
    Deferred,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::InPhase => "本期",
            Scope::Deferred => "延期",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlanItem {
    pub id: &'static str,
    pub title: &'static str,
    pub priority: Priority,
    pub scope: Scope,
    pub note: &'static str,
}

/// 本期台账。新增/调整条目只改这里，`selfcheck` 会立刻校验 P0 口径。
pub const PLAN: &[PlanItem] = &[
    PlanItem {
        id: "E-00",
        title: "审计价四字段与有效计价基准（BR-14 回溯重算）",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "本次已落地：ct_contract 四列 + effective_base + 比率断言",
    },
    PlanItem {
        id: "E-02",
        title: "超付拦截（BR-01 挂在保存拨款单）",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "本次已落地：PaymentQuota::check_new_payment",
    },
    PlanItem {
        id: "E-05",
        title: "金额负数按财务惯例括号显示",
        priority: Priority::P1,
        scope: Scope::InPhase,
        note: "本次已落地：Money::format_yuan",
    },
    PlanItem {
        id: "E-07",
        title: "报表导出与打印（xlsx）",
        priority: Priority::P2,
        scope: Scope::Deferred,
        note: "依赖打印模板与列配置，二期",
    },
    PlanItem {
        id: "E-16",
        title: "历史数据迁移与清理",
        priority: Priority::P1,
        scope: Scope::Deferred,
        note: "以附录 E 校正为准（原误标 P0），回写 11.8.1",
    },
    PlanItem {
        id: "FR-CT-01",
        title: "合同登记（含付款条款必录）",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "payment_terms 已进实体与校验",
    },
    PlanItem {
        id: "FR-CT-05",
        title: "审计价登记与修改留痕（BR-16）",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "本次已落地：sys_meta 历史数组",
    },
    PlanItem {
        id: "FR-PAY-06",
        title: "拨款单登记与作废",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "单级登记，无申请单、无审批（H3）",
    },
    PlanItem {
        id: "FR-RPT-02",
        title: "合同会计分类汇总",
        priority: Priority::P0,
        scope: Scope::InPhase,
        note: "按 ct_category（会计分类）汇总，与合同类别分离（H4）",
    },
    PlanItem {
        id: "FR-SYS-05",
        title: "RBAC 权限模型",
        priority: Priority::P1,
        scope: Scope::Deferred,
        note: "原标 P0，按 M5 降级；本期单机单用户",
    },
    PlanItem {
        id: "FR-SYS-07",
        title: "数据权限（项目级隔离）",
        priority: Priority::P1,
        scope: Scope::Deferred,
        note: "依赖用户体系，随 FR-SYS-05 延后",
    },
    PlanItem {
        id: "H5-REVERSAL",
        title: "红冲单据（REVERSAL/REVERSED）",
        priority: Priority::P2,
        scope: Scope::Deferred,
        note: "本期以「作废 + 重登」替代，状态枚举与列已移除",
    },
    PlanItem {
        id: "H3-APPLICATION",
        title: "支付申请与三级审批",
        priority: Priority::P2,
        scope: Scope::Deferred,
        note: "随 R-05 移出，DDL 保留为二期结构预留",
    },
];

/// 校验 P0 与本期范围的一致性，返回违例描述（空 = 通过）。
pub fn audit_p0_consistency() -> Vec<String> {
    let mut out = Vec::new();
    for it in PLAN {
        if it.priority == Priority::P0 && it.scope == Scope::Deferred {
            out.push(format!(
                "{} {} 标为 P0 但属于延期范围（{}）",
                it.id, it.title, it.note
            ));
        }
        if it.id.trim().is_empty() || it.title.trim().is_empty() {
            out.push("存在 id 或标题为空的台账条目".to_string());
        }
    }
    // 编号唯一
    let mut ids: Vec<&str> = PLAN.iter().map(|i| i.id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    if ids.len() != before {
        out.push("台账编号重复".to_string());
    }
    out
}

pub fn p0_items() -> impl Iterator<Item = &'static PlanItem> {
    PLAN.iter().filter(|i| i.priority == Priority::P0)
}

pub fn find(id: &str) -> Option<&'static PlanItem> {
    PLAN.iter().find(|i| i.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p0_必须在本期范围内() {
        assert!(audit_p0_consistency().is_empty(), "{:?}", audit_p0_consistency());
    }

    #[test]
    fn 白名单条目已降级() {
        for id in ["FR-SYS-05", "FR-SYS-07", "E-16"] {
            let it = find(id).expect("台账应包含该条目");
            assert_ne!(it.priority, Priority::P0, "{id} 应为 P1（M5）");
        }
        assert_eq!(find("E-16").unwrap().priority, Priority::P1);
    }

    #[test]
    fn 本期_p0_条目数稳定() {
        assert_eq!(p0_items().count(), 6);
    }
}
