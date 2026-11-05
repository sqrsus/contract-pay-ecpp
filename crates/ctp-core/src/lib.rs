//! 工程合同台账与拨款管理平台 —— 领域层。
//!
//! 本层只依赖标准库，不接触数据库与界面，便于把「口径」写成可执行的断言。
//! 口径的单一事实来源见 `spec` 模块与仓库根目录 `docs/口径主表.md`。

pub mod amount_cn;
pub mod attachment;
pub mod contract;
pub mod date;
pub mod digest;
pub mod dict;
pub mod error;
pub mod ids;
pub mod ledger;
pub mod money;
pub mod payment;
pub mod project;
pub mod quota;
pub mod rate;
pub mod refs;
pub mod scope;

pub use amount_cn::rmb_uppercase;
pub use attachment::{Attachment, AttachmentKind, MAX_ATTACHMENT_BYTES};
pub use contract::{Contract, ContractStatus, ContractType, PricingMethod, BUSINESS_FIELDS};
pub use error::{DomainError, Result};
pub use ledger::{CategoryRow, ContractSummary, LedgerRow, PeriodRow};
pub use money::Money;
pub use payment::{PayMethod, Payment, PaymentStatus};
pub use quota::{BreachLevel, PaymentQuota};
pub use rate::Rate;
