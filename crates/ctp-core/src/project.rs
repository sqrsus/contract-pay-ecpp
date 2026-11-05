use crate::date::validate_iso_date;
use crate::error::{DomainError, Result};

/// 建设项目（`prj_project`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub project_code: String,
    pub project_name: String,
    pub remark: Option<String>,
    pub is_deleted: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// 单项/单位工程（`prj_building`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Building {
    pub id: String,
    pub project_id: String,
    pub building_name: String,
    /// 建筑面积 ×100（保留两位小数，单位 ㎡）
    pub floor_area_x100: i64,
    pub structure_type: Option<String>,
    pub floor_count: i64,
    pub is_deleted: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// 参建单位角色（4.5.2：一行一角色）。
///
/// `Owner` 为发包人（甲方）保留位：甲方与承包单位共用一套单位档案，
/// 靠角色区分，避免同名单位在两张表里各存一份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContractorRole {
    #[default]
    Construct,
    Supervise,
    Design,
    Survey,
    Supplier,
    Owner,
    Other,
    /// 招标代理（新建合同页「招标代理」自动建档时使用的角色）
    Agent,
}

impl ContractorRole {
    pub fn code(self) -> &'static str {
        match self {
            ContractorRole::Construct => "CONSTRUCT",
            ContractorRole::Supervise => "SUPERVISE",
            ContractorRole::Design => "DESIGN",
            ContractorRole::Survey => "SURVEY",
            ContractorRole::Supplier => "SUPPLIER",
            ContractorRole::Owner => "OWNER",
            ContractorRole::Other => "OTHER",
            ContractorRole::Agent => "AGENT",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContractorRole::Construct => "施工",
            ContractorRole::Supervise => "监理",
            ContractorRole::Design => "设计",
            ContractorRole::Survey => "勘察",
            ContractorRole::Supplier => "供应商",
            ContractorRole::Owner => "业主",
            ContractorRole::Other => "其他",
            ContractorRole::Agent => "招标代理",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "CONSTRUCT" => ContractorRole::Construct,
            "SUPERVISE" => ContractorRole::Supervise,
            "DESIGN" => ContractorRole::Design,
            "SURVEY" => ContractorRole::Survey,
            "SUPPLIER" => ContractorRole::Supplier,
            "OWNER" => ContractorRole::Owner,
            "OTHER" => ContractorRole::Other,
            "AGENT" => ContractorRole::Agent,
            _ => return None,
        })
    }
}

/// 参建单位（`org_contractor`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contractor {
    pub id: String,
    pub contractor_name: String,
    pub role_type: ContractorRole,
    pub credit_code: Option<String>,
    pub bank_name: Option<String>,
    pub bank_account: Option<String>,
    pub remark: Option<String>,
    pub is_deleted: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// 单位联系人（`org_contact`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    pub contractor_id: String,
    pub contact_name: String,
    pub phone: Option<String>,
    pub is_primary: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

impl Project {
    pub fn validate(&self) -> Result<()> {
        if self.project_code.trim().is_empty() {
            return Err(DomainError::MissingField("项目编码"));
        }
        if self.project_name.trim().is_empty() {
            return Err(DomainError::MissingField("项目名称"));
        }
        Ok(())
    }
}

impl Contractor {
    pub fn validate(&self) -> Result<()> {
        if self.contractor_name.trim().is_empty() {
            return Err(DomainError::MissingField("单位名称"));
        }
        Ok(())
    }
}

impl Contact {
    pub fn validate(&self) -> Result<()> {
        if self.contact_name.trim().is_empty() {
            return Err(DomainError::MissingField("联系人"));
        }
        if let Some(p) = &self.phone {
            let digits = p.chars().filter(|c| c.is_ascii_digit()).count();
            if digits < 7 {
                return Err(DomainError::Invariant(format!("电话格式可疑：{p}")));
            }
        }
        Ok(())
    }
}

/// 期间账的日期键校验（`YYYY-MM`），报表分组用。
pub fn is_period_key(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(|c| c.is_ascii_digit())
        && b[5..].iter().all(|c| c.is_ascii_digit())
}

pub fn validate_sign_date(s: &str) -> Result<()> {
    validate_iso_date(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 单位与项目必填校验() {
        let mut p = Project::default();
        assert!(p.validate().is_err());
        p.project_code = "XAJT".into();
        p.project_name = "某市政道路".into();
        assert!(p.validate().is_ok());

        let mut c = Contractor::default();
        assert!(c.validate().is_err());
        c.contractor_name = "某建工集团".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn 期间键格式() {
        assert!(is_period_key("2026-11"));
        assert!(!is_period_key("2026-1"));
        assert!(!is_period_key("2026/11"));
    }
}
