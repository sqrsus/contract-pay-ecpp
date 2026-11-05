//! 应用服务层：合同登记、审计价登记、金额编辑留痕、拨款登记与作废、口径对账、
//! 报表导出与付款打印。
//!
//! 所有写操作都在 `BEGIN IMMEDIATE` 事务内完成，额度校验与流水号自增同事务，
//! 避免「先查后写」的并发超付（BR-01 的落点在保存拨款单，见 6.2.4）。

pub mod checks;
pub mod demo;
pub mod history;
pub mod paper;
pub mod print;
pub mod report;
pub mod service;
pub mod xlsx;

#[cfg(windows)]
pub mod gdi;

pub use paper::Voucher;
pub use print::{PaymentPrintData, Preview, PrintError, PAYMENT_TEMPLATE};
pub use report::{Cell, ColumnKind, ReportColumn, ReportKind, ReportResult};
pub use service::{
    create_contract, edit_contract_amount, export_report, payment_print_data, print_vouchers,
    quota_of, quota_on, reconcile, register_audit_price, register_payment, report_of,
    void_contract, void_payment, NewContract, NewPayment, RateMismatch,
};

// ---------------------------------------------------------------- 打印门面

/// 渲染一页预览位图（与出纸共用同一份版面）。
#[cfg(windows)]
pub fn render_print_page(v: &Voucher, page: usize, dpi: f32) -> Result<Preview, PrintError> {
    gdi::render_page(v, page, dpi)
}

#[cfg(not(windows))]
pub fn render_print_page(_v: &Voucher, _page: usize, _dpi: f32) -> Result<Preview, PrintError> {
    Err(PrintError::Print("打印功能仅支持 Windows".into()))
}

/// 弹系统打印对话框并输出；返回实际页数，`Ok(0)` 表示用户取消。
///
/// `owner` 是主窗口句柄，0 表示不挂 owner。
#[cfg(windows)]
pub fn print_vouchers_now(owner: isize, vs: &[Voucher], name: &str) -> Result<usize, PrintError> {
    gdi::print_vouchers(owner as *mut std::ffi::c_void, vs, name)
}

#[cfg(not(windows))]
pub fn print_vouchers_now(_owner: isize, _vs: &[Voucher], _name: &str) -> Result<usize, PrintError> {
    Err(PrintError::Print("打印功能仅支持 Windows".into()))
}
