mod data;
mod engine;
mod invariants;
mod naive;
mod ops;
mod oracle;
mod query;
mod schema;

pub use data::{gen_row, gen_rows, Domain};
pub use engine::{Engine, EngineError};
pub use invariants::check_invariants;
pub use naive::NaiveRecompute;
pub use ops::{gen_ops, Op};
pub use oracle::recompute_via_sqlite;
pub use query::{enumerate, Agg, AggFn, Predicate, ViewQuery};
pub use schema::{Column, ColumnType, Schema};
