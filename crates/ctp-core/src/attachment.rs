//! 合同附件（`ct_contract_attachment`）。
//!
//! 本期四类：中标通知书、合同、中期结算材料、审计报告。合同为正本必传项，
//! 其余三类可选；每类仅保留一份，替换即覆盖原件元数据。
//!
//! 文件本体以 BLOB 存在库里，因此这里的「元数据」结构刻意不带 `content`：
//! 列表页只查元数据，取字节单独走 `repo::attachment_bytes`，避免翻列表时把
//! 几十兆的字节读进内存。

/// 附件分类。`code()` 落库，`label()` 上界面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentKind {
    /// 中标通知书
    BidNotice,
    /// 合同（正本，必传）
    Contract,
    /// 中期结算材料
    ProgressSettlement,
    /// 审计报告
    AuditReport,
}

impl AttachmentKind {
    pub const ALL: [AttachmentKind; 4] = [
        AttachmentKind::BidNotice,
        AttachmentKind::Contract,
        AttachmentKind::ProgressSettlement,
        AttachmentKind::AuditReport,
    ];

    pub fn code(self) -> &'static str {
        match self {
            AttachmentKind::BidNotice => "BID_NOTICE",
            AttachmentKind::Contract => "CONTRACT",
            AttachmentKind::ProgressSettlement => "PROGRESS_SETTLEMENT",
            AttachmentKind::AuditReport => "AUDIT_REPORT",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AttachmentKind::BidNotice => "中标通知书",
            AttachmentKind::Contract => "合同",
            AttachmentKind::ProgressSettlement => "中期结算材料",
            AttachmentKind::AuditReport => "审计报告",
        }
    }

    /// 是否必传。仅合同正本为必传项。
    pub fn is_required(self) -> bool {
        matches!(self, AttachmentKind::Contract)
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "BID_NOTICE" => AttachmentKind::BidNotice,
            "CONTRACT" => AttachmentKind::Contract,
            "PROGRESS_SETTLEMENT" => AttachmentKind::ProgressSettlement,
            "AUDIT_REPORT" => AttachmentKind::AuditReport,
            _ => return None,
        })
    }
}

/// 附件元数据（不含字节）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub contract_id: String,
    pub kind_code: String,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub size_bytes: i64,
    /// 原文件字节的 SHA-256（小写十六进制），用于判断「是否同一份文件」。
    pub sha256: String,
    pub remark: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
    pub version: i64,
}

impl Attachment {
    pub fn kind(&self) -> Option<AttachmentKind> {
        AttachmentKind::from_code(&self.kind_code)
    }

    /// 人类可读的体积：B / KB / MB。
    pub fn size_label(&self) -> String {
        let n = self.size_bytes.max(0);
        if n < 1024 {
            format!("{n} B")
        } else if n < 1024 * 1024 {
            format!("{:.0} KB", n as f64 / 1024.0)
        } else {
            format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
        }
    }
}

/// 单文件上限：50 MB。超过则拒绝落库，避免数据库无上限膨胀。
pub const MAX_ATTACHMENT_BYTES: i64 = 50 * 1024 * 1024;

/// 单份合同的附件总量上限：20 × 单文件上限（1 GB，R-39）。
///
/// 附件本体以 BLOB 存在数据库里，单文件上限只能挡住「一次传一个超大文件」，
/// 挡不住「反复传满额文件」把库撑爆；这里按合同维度再设一道总量闸门。
pub const MAX_ATTACHMENT_TOTAL_BYTES: i64 = 20 * MAX_ATTACHMENT_BYTES;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 附件分类编码往返() {
        for k in AttachmentKind::ALL {
            assert_eq!(AttachmentKind::from_code(k.code()), Some(k));
            assert!(!k.label().is_empty());
        }
        assert_eq!(AttachmentKind::from_code("UNKNOWN"), None);
    }

    #[test]
    fn 仅合同必传() {
        for k in AttachmentKind::ALL {
            assert_eq!(k.is_required(), k == AttachmentKind::Contract);
        }
    }

    #[test]
    fn 体积显示() {
        let mut a = Attachment {
            size_bytes: 900,
            ..Default::default()
        };
        assert_eq!(a.size_label(), "900 B");
        a.size_bytes = 2048;
        assert_eq!(a.size_label(), "2 KB");
        a.size_bytes = 3 * 1024 * 1024;
        assert_eq!(a.size_label(), "3.0 MB");
    }
}
