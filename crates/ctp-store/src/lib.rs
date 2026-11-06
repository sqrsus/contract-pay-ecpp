//! 持久化层：SQLite 建表、视图与仓储。
//!
//! 设计原则（对应评审意见）：
//! * **口径唯一**：已付净额、支付比率只有一处实现（`schema::paid_net_subquery` /
//!   `schema::rate_bp_expr`），仓储与视图复用同一段 SQL（H2 / 观点一「口径主表」）；
//! * **本期裁剪**：只建本期真正使用的 12 张表，申请单/审批/分摊表不建（H3）；
//! * **结构自检**：`repo::audit_column_violations` 把 DB-06 的「每表必备审计列」变成可执行断言（M4）。

pub mod audit;
pub mod db;
pub mod error;
pub mod repo;
pub mod schema;
pub mod seed;

pub use error::{Result, StoreError};
