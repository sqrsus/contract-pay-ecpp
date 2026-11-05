//! 应用服务层：合同登记、审计价登记、金额编辑留痕、拨款登记与作废、口径对账、
//! 报表导出与付款打印数据。
//!
//! 所有写操作都在 `BEGIN IMMEDIATE` 事务内完成，额度校验与流水号自增同事务，
//! 避免「先查后写」的并发超付（BR-01 的落点在保存拨款单，见 6.2.4）。

pub mod checks;
pub mod demo;
pub mod docx;
pub mod history;
pub mod pdf;
pub mod print;
pub mod report;
pub mod service;
pub mod xlsx;

pub use print::{PaymentPrintData, PrintError, PAYMENT_TEMPLATE};
pub use report::{Cell, ColumnKind, ReportColumn, ReportKind, ReportResult};
pub use service::{
    create_contract, edit_contract_amount, export_report, print_payment_docx, quota_of, quota_on,
    reconcile, register_audit_price, register_payment, report_of, void_contract, void_payment,
    NewContract, NewPayment, RateMismatch,
};
