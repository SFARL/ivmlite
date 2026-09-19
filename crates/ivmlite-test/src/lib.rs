mod buggy;
mod data;
mod differential;
mod engine;
mod invariants;
mod naive;
mod ops;
mod oracle;
mod query;
mod regression;
mod schema;
mod shrink;

pub use buggy::{NoRetractionEngine, TransientDriftEngine};
pub use data::{gen_row, gen_rows, Domain};
pub use differential::{
    check_batch_invariance, gen_case, run, seed_range, Batching, Failure, TestCase,
};
pub use engine::{Engine, EngineError};
pub use invariants::check_invariants;
pub use naive::NaiveRecompute;
pub use ops::{gen_ops, Op};
pub use oracle::recompute_via_sqlite;
pub use query::{enumerate, Agg, AggFn, Predicate, ViewQuery};
pub use regression::{load_regressions, save_regression};
pub use schema::{Column, ColumnType, Schema};
pub use shrink::{is_legal, shrink};
