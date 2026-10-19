mod buggy;
mod compare;
mod data;
pub mod demand_cases;
pub mod demo_bench;
mod differential;
mod engine;
mod exact_reference;
mod extension;
pub mod fluxflow;
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
pub use compare::compare_with_oracles;
pub use data::{
    gen_database, gen_database_with_swapped_right_table, gen_initial, gen_real_numeric_database,
    gen_row, gen_rows, value_pool, Domain, Pool,
};
pub use differential::{
    check_batch_invariance, gen_case, gen_case_with_query, run, run_outcome, seed_range, Batching,
    Failure, Outcome, TestCase,
};
pub use engine::{Engine, EngineError};
pub use exact_reference::{exact_mean, fsum, int_pieces, UNIT_ROUNDOFF};
pub use extension::{
    extension_library, extension_library_for, open_with_extension, open_with_extension_at, Profile,
    SqliteExtensionEngine,
};
pub use invariants::check_invariants;
pub use ivmlite_core::{
    Agg, AggFn, CmpOp, Column, ColumnType, Database, IncrementalEngine, Join, Predicate, Schema,
    ViewQuery,
};
pub use naive::{evaluate, Evaluated, NaiveRecompute, SqliteTerms};
pub use ops::{gen_ops, Op};
pub use oracle::recompute_via_sqlite;
pub use query::{enumerate, enumerate_database, enumerate_join, AGGREGATE_FUNCTIONS};
pub use regression::{load_regressions, save_regression};
pub use shrink::{is_legal, shrink};
pub use sql::{create_table_sql, view_query_to_sql};
