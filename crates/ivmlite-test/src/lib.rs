mod buggy;
mod data;
mod differential;
mod engine;
mod incremental;
mod invariants;
mod naive;
mod ops;
mod oracle;
mod query;
mod regression;
mod shrink;
mod sql;
#[cfg(test)]
mod test_support;

pub use buggy::{NoRetractionEngine, TransientDriftEngine};
pub use data::{gen_database, gen_initial, gen_row, gen_rows, Domain};
pub use differential::{
    check_batch_invariance, gen_case, gen_case_with_query, run, seed_range, Batching, Failure,
    TestCase,
};
pub use engine::{Engine, EngineError};
pub use invariants::check_invariants;
pub use ivmlite_core::{
    Agg, AggFn, Column, ColumnType, Database, IncrementalEngine, Join, Predicate, Schema, ViewQuery,
};
pub use naive::NaiveRecompute;
pub use ops::{gen_ops, Op};
pub use oracle::recompute_via_sqlite;
pub use query::{enumerate, enumerate_database, enumerate_join};
pub use regression::{load_regressions, save_regression};
pub use shrink::{is_legal, shrink};
pub use sql::{create_table_sql, view_query_to_sql};
