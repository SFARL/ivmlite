//! Execution of exported M2a traces. Generation and state inspection are untimed.
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use ivmlite_test::demo_bench::{multiset_difference, sorted_rows, Summary};
use ivmlite_test::open_with_extension_at;
use ivmlite_workload::m2a::{Fixture, Mutation, Shape, Table};
use rusqlite::types::Value;
use rusqlite::{params, Connection, Statement, StatementStatus};
use serde::{Deserialize, Serialize};

use crate::m2a_state::{
    capture_pages, capture_snapshot, validate_idle, validate_snapshot, PageMetrics, Snapshot,
};
use crate::m2a_tracecheck::TraceReplay;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const VIEW: &str = "mv_m2a";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    NoMaintenance,
    Recompute,
    CoveringRecompute,
    Ivmlite,
}
pub const MODES: [Mode; 4] = [
    Mode::NoMaintenance,
    Mode::Recompute,
    Mode::CoveringRecompute,
    Mode::Ivmlite,
];
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoMaintenance => "no_maintenance",
            Self::Recompute => "recompute",
            Self::CoveringRecompute => "covering_recompute",
            Self::Ivmlite => "ivmlite",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundTiming {
    pub round: usize,
    pub apply_ms: f64,
    pub maintain_ms: Option<f64>,
    pub read_ms: Option<f64>,
    pub apply_plus_maintain_ms: Option<f64>,
    pub total_ms: Option<f64>,
}
impl RoundTiming {
    fn new(round: usize, apply: f64, maintain: Option<f64>, read: Option<f64>) -> Self {
        Self {
            round,
            apply_ms: apply,
            maintain_ms: maintain,
            read_ms: read,
            apply_plus_maintain_ms: maintain.map(|m| apply + m),
            total_ms: maintain.zip(read).map(|(m, r)| apply + m + r),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteadyState {
    pub apply_ms: f64,
    pub maintain_ms: Option<f64>,
    pub read_ms: Option<f64>,
    pub apply_plus_maintain_ms: Option<f64>,
    pub total_ms: Option<f64>,
}
pub fn steady_state(rounds: &[RoundTiming]) -> Result<SteadyState> {
    if rounds.is_empty() {
        return Err("M2a requires measured rounds".into());
    }
    let median = |v: Vec<f64>| {
        Summary::of(&v)
            .map(|s| s.median)
            .ok_or("missing M2a rounds")
    };
    let optional = |f: fn(&RoundTiming) -> Option<f64>| -> Result<Option<f64>> {
        let values: Vec<_> = rounds.iter().filter_map(f).collect();
        if values.is_empty() {
            Ok(None)
        } else if values.len() != rounds.len() {
            Err("inconsistent round metrics".into())
        } else {
            Ok(Some(median(values)?))
        }
    };
    Ok(SteadyState {
        apply_ms: median(rounds.iter().map(|r| r.apply_ms).collect())?,
        maintain_ms: optional(|r| r.maintain_ms)?,
        read_ms: optional(|r| r.read_ms)?,
        apply_plus_maintain_ms: optional(|r| r.apply_plus_maintain_ms)?,
        total_ms: optional(|r| r.total_ms)?,
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    pub mode: Mode,
    pub bootstrap_ms: f64,
    pub warmup: RoundTiming,
    pub measured: Vec<RoundTiming>,
    pub summary: SteadyState,
    pub base_pages: PageMetrics,
    pub bootstrap_pages: PageMetrics,
    pub final_pages: PageMetrics,
    pub plan: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub phase: String,
    pub round: Option<usize>,
    pub snapshot: Snapshot,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStats {
    pub index: usize,
    pub table: Table,
    pub primary_key: i64,
    pub kind: String,
    pub anchor_group: Option<i64>,
    pub matched_partners: usize,
    pub matched_hot_lhs_rows: usize,
    pub matched_distinct_groups: usize,
    pub matched_hot_groups: usize,
    /// Rows SQLite reported changed by the operation's statement during the correctness pass.
    pub rows_changed: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetStats {
    pub round: usize,
    pub operations: usize,
    pub target_selections: usize,
    pub hot_anchor_selections: usize,
    pub distinct_primary_keys: usize,
    pub repeated_primary_key_touches: usize,
    pub repeated_existing_targets: usize,
    pub target_groups: BTreeMap<i64, usize>,
    pub rhs_matched_lhs_rows: usize,
    pub rhs_matched_hot_lhs_rows: usize,
    pub rhs_affected_group_visits: usize,
    pub rhs_hot_group_visits: usize,
    pub matched_partners_min: usize,
    pub matched_partners_max: usize,
    pub matched_partners_total: usize,
    pub per_operation: Vec<OperationStats>,
}

pub fn open(mode: Mode, library: &Path) -> Result<Connection> {
    let c = if mode == Mode::Ivmlite {
        open_with_extension_at(None, library)?
    } else {
        Connection::open_in_memory()?
    };
    apply_pragmas(&c)?;
    Ok(c)
}
const PRAGMA_SETUP: &str = "PRAGMA page_size=4096; PRAGMA cache_size=-65536; PRAGMA temp_store=MEMORY; PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON;";
/// What `PRAGMA <name>` must report once `PRAGMA_SETUP` has run (SQLite reports enumerations
/// numerically: temp_store MEMORY is 2, synchronous OFF is 0).
const PINNED_PRAGMAS: [(&str, &str); 7] = [
    ("page_size", "4096"),
    ("cache_size", "-65536"),
    ("temp_store", "2"),
    ("journal_mode", "memory"),
    ("synchronous", "0"),
    ("foreign_keys", "1"),
    ("recursive_triggers", "1"),
];
fn apply_pragmas(c: &Connection) -> Result<()> {
    c.execute_batch(PRAGMA_SETUP)?;
    check_pragmas(&read_pragmas(c)?)
}
/// The value each pinned pragma currently has on `c`, read with `PRAGMA <name>`.
pub fn read_pragmas(c: &Connection) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, _) in PINNED_PRAGMAS {
        let value = match c.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, Value>(0))? {
            Value::Integer(i) => i.to_string(),
            Value::Text(t) => t,
            other => return Err(format!("PRAGMA {name} returned {other:?}").into()),
        };
        out.insert(name.to_owned(), value);
    }
    Ok(out)
}
pub fn check_pragmas(read: &BTreeMap<String, String>) -> Result<()> {
    for (name, pinned) in PINNED_PRAGMAS {
        let actual = read.get(name).map(String::as_str);
        if actual != Some(pinned) {
            return Err(format!("M2a pragma {name} is {actual:?}, pinned to {pinned}").into());
        }
    }
    Ok(())
}
pub fn seed(c: &Connection, fixture: &Fixture) -> Result<()> {
    c.execute_batch(fixture.cell.schema_ddl())?;
    let tx = c.unchecked_transaction()?;
    for (&table, rows) in &fixture.tables {
        let mut s = tx.prepare(insert_sql(table))?;
        for r in rows {
            match table {
                Table::Events => {
                    s.execute(params![r.id, r.g, r.amount])?;
                }
                Table::Lhs => {
                    s.execute(params![r.id, r.k, r.g])?;
                }
                Table::Rhs => {
                    s.execute(params![r.id, r.k, r.amount])?;
                }
            }
        }
    }
    tx.commit()?;
    c.execute_batch(fixture.cell.ordinary_indexes())?;
    Ok(())
}
fn insert_sql(t: Table) -> &'static str {
    match t {
        Table::Events => "INSERT INTO events(id,g,amount) VALUES(?1,?2,?3)",
        Table::Lhs => "INSERT INTO lhs(id,k,g) VALUES(?1,?2,?3)",
        Table::Rhs => "INSERT INTO rhs(id,k,amount) VALUES(?1,?2,?3)",
    }
}
fn update_sql(t: Table) -> &'static str {
    match t {
        Table::Events => "UPDATE events SET g=?2,amount=?3 WHERE id=?1",
        Table::Lhs => "UPDATE lhs SET k=?2,g=?3 WHERE id=?1",
        Table::Rhs => "UPDATE rhs SET k=?2,amount=?3 WHERE id=?1",
    }
}
fn delete_sql(t: Table) -> String {
    format!("DELETE FROM {} WHERE id=?1", t.name())
}
fn indexed_query(f: &Fixture) -> String {
    match f.cell.shape {Shape::Single=>"SELECT g, COUNT(*), SUM(amount) FROM events INDEXED BY events_g_amount GROUP BY g".into(),Shape::Join=>"SELECT lhs.g, COUNT(*), SUM(rhs.amount) FROM lhs INDEXED BY lhs_g_k JOIN rhs INDEXED BY rhs_k_amount ON lhs.k = rhs.k GROUP BY lhs.g".into()}
}
fn query_plan(c: &Connection, query: &str) -> Result<Vec<String>> {
    let mut statement = c.prepare(&format!("EXPLAIN QUERY PLAN {query}"))?;
    let unbound = vec![Value::Null; statement.parameter_count()];
    let plan = statement
        .query_map(rusqlite::params_from_iter(unbound), |r| {
            r.get::<_, String>(3)
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(plan)
}
pub fn plan_text(plan: &[String]) -> String {
    plan.join("\n")
}
/// A plan that never uses an index on the join key would make the recompute baseline a nested
/// full scan, so the comparison against it would not be a fair baseline.
pub fn assert_ordinary_recompute_plan(shape: Shape, plan: &str) -> Result<()> {
    if shape != Shape::Join {
        return Ok(());
    }
    let words: Vec<_> = plan.split_whitespace().collect();
    let names_index = |index: &str| {
        words.windows(3).any(|w| w == ["USING", "INDEX", index])
            || words
                .windows(4)
                .any(|w| w == ["USING", "COVERING", "INDEX", index])
    };
    let indexed = names_index("lhs_k") || names_index("rhs_k");
    if !indexed {
        return Err(format!("ordinary recompute plan uses no join key index: {plan}").into());
    }
    Ok(())
}
/// The plan of every UPDATE and DELETE the apply path prepares, keyed `<table>.<update|delete>`.
pub fn dml_plans(
    c: &Connection,
    tables: impl IntoIterator<Item = Table>,
) -> Result<BTreeMap<String, String>> {
    let mut plans = BTreeMap::new();
    for t in tables {
        for (kind, sql) in [
            ("update", update_sql(t).to_owned()),
            ("delete", delete_sql(t)),
        ] {
            plans.insert(
                format!("{}.{kind}", t.name()),
                plan_text(&query_plan(c, &sql)?),
            );
        }
    }
    Ok(plans)
}
/// Every UPDATE and DELETE must find its row by primary key, or the timed apply cost would grow
/// with table size.
pub fn assert_dml_plans(plans: &BTreeMap<String, String>) -> Result<()> {
    for (label, plan) in plans {
        if !plan.contains("USING INTEGER PRIMARY KEY") {
            return Err(format!("{label} plan is not a rowid lookup: {plan}").into());
        }
    }
    Ok(())
}
/// Query plans and connection settings, captured on a data-free copy of a fixture's schema
/// (SQLite plans without ANALYZE statistics do not depend on row counts).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plans {
    pub view_sql_reference: String,
    pub ordinary_recompute: String,
    pub covering_recompute: String,
    pub dml: BTreeMap<String, String>,
}
pub struct Provenance {
    pub pragmas: BTreeMap<String, String>,
    pub plans: Plans,
}
pub fn probe_provenance(f: &Fixture) -> Result<Provenance> {
    let c = Connection::open_in_memory()?;
    c.execute_batch(PRAGMA_SETUP)?;
    let pragmas = read_pragmas(&c)?;
    c.execute_batch(f.cell.schema_ddl())?;
    c.execute_batch(f.cell.ordinary_indexes())?;
    #[cfg(test)]
    if DROP_ORDINARY_INDEXES.with(std::cell::Cell::get) {
        c.execute_batch("DROP INDEX IF EXISTS lhs_k; DROP INDEX IF EXISTS rhs_k;")?;
    }
    let view_sql_reference = plan_text(&query_plan(&c, f.cell.view_sql())?);
    let dml = dml_plans(&c, f.tables.keys().copied())?;
    c.execute_batch(f.cell.covering_indexes())?;
    let covering_recompute = plan_text(&query_plan(&c, &indexed_query(f))?);
    Ok(Provenance {
        pragmas,
        plans: Plans {
            ordinary_recompute: view_sql_reference.clone(),
            view_sql_reference,
            covering_recompute,
            dml,
        },
    })
}
/// The plan checks that must hold before any timing starts.
pub fn assert_plans(plans: &Plans, shape: Shape) -> Result<()> {
    assert_ordinary_recompute_plan(shape, &plans.ordinary_recompute)?;
    assert_dml_plans(&plans.dml)
}
#[cfg(test)]
thread_local! {
    /// Test-only: makes `probe_provenance` capture plans without the join key indexes.
    pub(crate) static DROP_ORDINARY_INDEXES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(crate) fn bootstrap(c: &Connection, f: &Fixture, mode: Mode) -> Result<f64> {
    let start = Instant::now();
    if mode == Mode::CoveringRecompute {
        c.execute_batch(f.cell.covering_indexes())?;
    }
    match mode {
        Mode::Ivmlite => c.execute_batch(&format!(
            "CREATE VIRTUAL TABLE {VIEW} USING ivm('{}')",
            f.cell.view_sql().replace('\'', "''")
        ))?,
        Mode::Recompute | Mode::CoveringRecompute => {
            c.execute_batch("CREATE TABLE m2a_result(g INTEGER NOT NULL,n INTEGER NOT NULL,s INTEGER NOT NULL) STRICT")?;
            let q = if mode == Mode::CoveringRecompute {
                indexed_query(f)
            } else {
                f.cell.view_sql().into()
            };
            c.execute(&format!("INSERT INTO m2a_result {q}"), [])?;
        }
        Mode::NoMaintenance => {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
/// A row of an oracle comparison; every M2a result column is an INTEGER or NULL.
type OracleRow = Vec<Option<i64>>;
/// A result that is not the same multiset of rows as its oracle, with the full difference.
#[derive(Debug)]
pub struct MultisetMismatch {
    message: String,
    /// Rows only the oracle returned, sorted, duplicates kept.
    pub expected_only: Vec<OracleRow>,
    /// Rows only the checked result returned, sorted, duplicates kept.
    pub actual_only: Vec<OracleRow>,
}
impl MultisetMismatch {
    pub fn with_context(mut self, context: &str) -> Self {
        self.message = format!("{context}: {}", self.message);
        self
    }
}
impl std::fmt::Display for MultisetMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for MultisetMismatch {}
/// The rows only in `expected` and the rows only in `actual`, each sorted, duplicates kept.
pub fn symmetric_difference<T: Ord + Clone>(expected: &[T], actual: &[T]) -> (Vec<T>, Vec<T>) {
    let (mut expected, mut actual) = (expected.to_vec(), actual.to_vec());
    expected.sort();
    actual.sort();
    let (mut expected_only, mut actual_only) = (Vec::new(), Vec::new());
    let (mut e, mut a) = (0, 0);
    while e < expected.len() || a < actual.len() {
        match (expected.get(e), actual.get(a)) {
            (Some(x), Some(y)) if x == y => {
                e += 1;
                a += 1;
            }
            (Some(x), Some(y)) if x < y => {
                expected_only.push(x.clone());
                e += 1;
            }
            (Some(_), Some(y)) => {
                actual_only.push(y.clone());
                a += 1;
            }
            (Some(x), None) => {
                expected_only.push(x.clone());
                e += 1;
            }
            (None, Some(y)) => {
                actual_only.push(y.clone());
                a += 1;
            }
            (None, None) => unreachable!("the loop ends when both sides are consumed"),
        }
    }
    (expected_only, actual_only)
}
fn oracle_rows(rows: &[Vec<Value>]) -> std::result::Result<Vec<OracleRow>, String> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    Value::Integer(i) => Ok(Some(*i)),
                    Value::Null => Ok(None),
                    other => Err(format!(
                        "oracle comparison got a non-integer value {other:?}"
                    )),
                })
                .collect()
        })
        .collect()
}
/// `Ok` when `actual_sql` returns the same multiset of rows as `oracle_sql`.
pub fn oracle_check(
    c: &Connection,
    actual_sql: &str,
    oracle_sql: &str,
) -> std::result::Result<(), MultisetMismatch> {
    let query_failed = |why: String| MultisetMismatch {
        message: why,
        expected_only: Vec::new(),
        actual_only: Vec::new(),
    };
    let actual = sorted_rows(c, actual_sql).map_err(|e| query_failed(e.to_string()))?;
    let expected = sorted_rows(c, oracle_sql).map_err(|e| query_failed(e.to_string()))?;
    let Some(why) = multiset_difference(&actual, &expected) else {
        return Ok(());
    };
    let (expected_only, actual_only) = symmetric_difference(
        &oracle_rows(&expected).map_err(query_failed)?,
        &oracle_rows(&actual).map_err(query_failed)?,
    );
    Err(MultisetMismatch {
        message: format!("`{actual_sql}` diverged from `{oracle_sql}`: {why}"),
        expected_only,
        actual_only,
    })
}
fn verify(c: &Connection, f: &Fixture, mode: Mode) -> std::result::Result<(), MultisetMismatch> {
    if mode != Mode::NoMaintenance {
        let actual = if mode == Mode::Ivmlite {
            format!("SELECT * FROM {VIEW}")
        } else {
            "SELECT * FROM m2a_result".into()
        };
        oracle_check(c, &actual, f.cell.view_sql())?;
    }
    Ok(())
}
fn assert_not_reprepared(label: &str, s: &Statement<'_>) -> Result<()> {
    let n = s.get_status(StatementStatus::RePrepare);
    if n != 0 {
        return Err(format!("M2a {label}: {n} reprepare(s) in timed execution").into());
    }
    Ok(())
}
struct RowStatements<'c> {
    insert: Statement<'c>,
    delete: Statement<'c>,
    update: Statement<'c>,
}
pub struct Prepared<'c> {
    begin: Statement<'c>,
    commit: Statement<'c>,
    rows: BTreeMap<Table, RowStatements<'c>>,
    refresh: Option<Statement<'c>>,
    clear: Option<Statement<'c>>,
    recompute: Option<Statement<'c>>,
    read: Option<Statement<'c>>,
}
impl<'c> Prepared<'c> {
    pub fn new(c: &'c Connection, f: &Fixture, mode: Mode) -> Result<Self> {
        let mut rows = BTreeMap::new();
        for &t in f.tables.keys() {
            rows.insert(
                t,
                RowStatements {
                    insert: c.prepare(insert_sql(t))?,
                    delete: c.prepare(&delete_sql(t))?,
                    update: c.prepare(update_sql(t))?,
                },
            );
        }
        let (refresh, clear, recompute, read) = match mode {
            Mode::Ivmlite => (
                Some(c.prepare(&format!("INSERT INTO {VIEW}({VIEW}) VALUES('refresh')"))?),
                None,
                None,
                Some(c.prepare(&format!("SELECT * FROM {VIEW}"))?),
            ),
            Mode::Recompute | Mode::CoveringRecompute => {
                let q = if mode == Mode::CoveringRecompute {
                    indexed_query(f)
                } else {
                    f.cell.view_sql().into()
                };
                (
                    None,
                    Some(c.prepare("DELETE FROM m2a_result")?),
                    Some(c.prepare(&format!("INSERT INTO m2a_result {q}"))?),
                    Some(c.prepare("SELECT * FROM m2a_result")?),
                )
            }
            Mode::NoMaintenance => (None, None, None, None),
        };
        Ok(Self {
            begin: c.prepare("BEGIN IMMEDIATE")?,
            commit: c.prepare("COMMIT")?,
            rows,
            refresh,
            clear,
            recompute,
            read,
        })
    }
    pub fn apply(&mut self, ops: &[Mutation]) -> Result<f64> {
        self.run_apply(ops, None)
    }
    /// `apply` that also appends, per operation, the row count SQLite reported for it. Only for
    /// untimed passes: the push is not part of the measured apply.
    pub fn apply_counting(&mut self, ops: &[Mutation], changes: &mut Vec<u64>) -> Result<f64> {
        self.run_apply(ops, Some(changes))
    }
    fn run_apply(&mut self, ops: &[Mutation], mut changes: Option<&mut Vec<u64>>) -> Result<f64> {
        let start = Instant::now();
        self.begin.execute([])?;
        for op in ops {
            let statements = self
                .rows
                .get_mut(&op.table)
                .ok_or("trace names an unavailable table")?;
            let changed = match (&op.before, &op.after) {
                (None, Some(r)) => match op.table {
                    Table::Events => statements.insert.execute(params![r.id, r.g, r.amount])?,
                    Table::Lhs => statements.insert.execute(params![r.id, r.k, r.g])?,
                    Table::Rhs => statements.insert.execute(params![r.id, r.k, r.amount])?,
                },
                (Some(r), None) => statements.delete.execute([r.id])?,
                (Some(_), Some(r)) => match op.table {
                    Table::Events => statements.update.execute(params![r.id, r.g, r.amount])?,
                    Table::Lhs => statements.update.execute(params![r.id, r.k, r.g])?,
                    Table::Rhs => statements.update.execute(params![r.id, r.k, r.amount])?,
                },
                _ => return Err("empty mutation".into()),
            };
            if let Some(changes) = changes.as_deref_mut() {
                changes.push(changed as u64);
            }
            if changed != 1 {
                return Err(format!("{} mutation changed {changed} rows", op.table.name()).into());
            }
        }
        self.commit.execute([])?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        self.assert_apply_prepared()?;
        Ok(elapsed)
    }
    fn assert_apply_prepared(&self) -> Result<()> {
        assert_not_reprepared("BEGIN", &self.begin)?;
        assert_not_reprepared("COMMIT", &self.commit)?;
        for (&table, s) in &self.rows {
            for (label, stmt) in [
                ("INSERT", &s.insert),
                ("DELETE", &s.delete),
                ("UPDATE", &s.update),
            ] {
                assert_not_reprepared(&format!("{} {label}", table.name()), stmt)?;
            }
        }
        Ok(())
    }
    pub fn maintain(&mut self) -> Result<Option<f64>> {
        if self.refresh.is_none() && self.recompute.is_none() {
            return Ok(None);
        }
        let start = Instant::now();
        if let Some(s) = &mut self.refresh {
            s.execute([])?;
        }
        if let Some(s) = &mut self.clear {
            s.execute([])?;
        }
        if let Some(s) = &mut self.recompute {
            s.execute([])?;
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        for (label, s) in [
            ("refresh", &self.refresh),
            ("clear result", &self.clear),
            ("recompute", &self.recompute),
        ] {
            if let Some(s) = s {
                assert_not_reprepared(label, s)?;
            }
        }
        Ok(Some(elapsed))
    }
    pub fn read(&mut self) -> Result<Option<f64>> {
        let Some(s) = &mut self.read else {
            return Ok(None);
        };
        let start = Instant::now();
        let mut checksum = 0i64;
        {
            let mut cursor = s.query([])?;
            while let Some(row) = cursor.next()? {
                for i in 0..3 {
                    checksum = checksum.wrapping_add(row.get::<_, i64>(i)?);
                }
            }
        }
        black_box(checksum);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert_not_reprepared("read result", s)?;
        Ok(Some(elapsed))
    }
    fn round(&mut self, n: usize, ops: &[Mutation]) -> Result<RoundTiming> {
        let apply = self.apply(ops)?;
        let maintain = self.maintain()?;
        let read = self.read()?;
        Ok(RoundTiming::new(n, apply, maintain, read))
    }
}

pub fn time_fixture(f: &Fixture, mode: Mode, library: &Path) -> Result<Measurement> {
    f.validate()?;
    if f.config.warmup_rounds != 1 || f.batches.len() != 1 + f.config.measured_rounds {
        return Err("timing requires one warm-up and the configured measured rounds".into());
    }
    let c = open(mode, library)?;
    seed(&c, f)?;
    let mut warmup_trace = TraceReplay::new(f)?;
    warmup_trace
        .verify(&c)
        .map_err(|e| format!("bootstrap trace-prefix: {e}"))?;
    warmup_trace.apply(&f.batches[0])?;
    let mut final_trace = TraceReplay::new(f)?;
    for operations in &f.batches {
        final_trace.apply(operations)?;
    }
    let base_pages = capture_pages(&c)?;
    let bootstrap_ms = bootstrap(&c, f, mode)?;
    let bootstrap_pages = capture_pages(&c)?;
    verify(&c, f, mode)?;
    let query = if mode == Mode::CoveringRecompute {
        indexed_query(f)
    } else {
        f.cell.view_sql().into()
    };
    let recomputes = matches!(mode, Mode::Recompute | Mode::CoveringRecompute);
    let plan = if recomputes {
        query_plan(&c, &query)?
    } else {
        Vec::new()
    };
    if mode == Mode::Recompute {
        assert_ordinary_recompute_plan(f.cell.shape, &plan_text(&plan))?;
    }
    if mode == Mode::CoveringRecompute {
        let required = if f.cell.shape == Shape::Join {
            vec!["lhs_g_k", "rhs_k_amount"]
        } else {
            vec!["events_g_amount"]
        };
        for index in required {
            if !plan
                .iter()
                .any(|line| line.contains(&format!("COVERING INDEX {index}")))
            {
                return Err(format!("covering plan missing {index}: {plan:?}").into());
            }
        }
        oracle_check(&c, &query, f.cell.view_sql())?;
    }
    assert_dml_plans(&dml_plans(&c, f.tables.keys().copied())?)?;
    let mut prepared = Prepared::new(&c, f, mode)?;
    let warmup = prepared.round(0, &f.batches[0])?;
    warmup_trace
        .verify(&c)
        .map_err(|e| format!("warm-up trace-prefix: {e}"))?;
    verify(&c, f, mode)?;
    let mut measured = Vec::new();
    for (n, ops) in f.batches.iter().enumerate().skip(1) {
        let apply = prepared.apply(ops)?;
        let maintain = prepared.maintain()?;
        let read = prepared.read()?;
        measured.push(RoundTiming::new(n, apply, maintain, read));
    }
    verify(&c, f, mode)?;
    if mode == Mode::CoveringRecompute {
        oracle_check(&c, &query, f.cell.view_sql())?;
    }
    final_trace
        .verify(&c)
        .map_err(|e| format!("final trace-prefix: {e}"))?;
    let final_pages = capture_pages(&c)?;
    let summary = steady_state(&measured)?;
    Ok(Measurement {
        mode,
        bootstrap_ms,
        warmup,
        measured,
        summary,
        base_pages,
        bootstrap_pages,
        final_pages,
        plan,
    })
}

/// The checkpoints of a correctness pass and, per forward round and operation, the number of rows
/// SQLite reported changed by that operation's statement.
pub struct Correctness {
    pub checkpoints: Vec<Checkpoint>,
    pub rows_changed: Vec<Vec<u64>>,
}
pub fn correctness(f: &Fixture, library: &Path, inverse: bool) -> Result<Correctness> {
    f.validate()?;
    let c = open(Mode::Ivmlite, library)?;
    seed(&c, f)?;
    bootstrap(&c, f, Mode::Ivmlite)?;
    let mut replay = TraceReplay::new(f)?;
    replay
        .verify(&c)
        .map_err(|e| format!("bootstrap trace-prefix: {e}"))?;
    let join = f.cell.shape == Shape::Join;
    let initial = validate_snapshot(&c, join, VIEW)?;
    let signature = initial.retained_signature().clone();
    let mut checkpoints = vec![Checkpoint {
        phase: "bootstrap".into(),
        round: None,
        snapshot: initial.into_summary(),
    }];
    let mut p = Prepared::new(&c, f, Mode::Ivmlite)?;
    let mut rows_changed = Vec::new();
    for (round, ops) in f.batches.iter().enumerate() {
        let old = sorted_rows(&c, &format!("SELECT * FROM {VIEW}"))?;
        replay
            .apply(ops)
            .map_err(|e| format!("forward round {round}: {e}"))?;
        let mut changes = Vec::with_capacity(ops.len());
        p.apply_counting(ops, &mut changes)
            .map_err(|e| format!("forward round {round} apply: {e}"))?;
        rows_changed.push(changes);
        replay
            .verify(&c)
            .map_err(|e| format!("forward round {round} trace-prefix: {e}"))?;
        let stale = sorted_rows(&c, &format!("SELECT * FROM {VIEW}"))?;
        if let Some(diff) = multiset_difference(&stale, &old) {
            return Err(format!("round {round}: eager view update: {diff}").into());
        }
        checkpoints.push(Checkpoint {
            phase: "written".into(),
            round: Some(round),
            snapshot: capture_snapshot(&c, join, VIEW)?.into_summary(),
        });
        p.maintain()?;
        verify(&c, f, Mode::Ivmlite)
            .map_err(|e| e.with_context(&format!("forward round {round} oracle")))?;
        let maintained = validate_snapshot(&c, join, VIEW)?;
        let before_idle = maintained.clone();
        checkpoints.push(Checkpoint {
            phase: "refreshed".into(),
            round: Some(round),
            snapshot: maintained.into_summary(),
        });
        p.maintain()?;
        let idle = validate_snapshot(&c, join, VIEW)?;
        validate_idle(&before_idle, &idle)?;
        checkpoints.push(Checkpoint {
            phase: "idle".into(),
            round: Some(round),
            snapshot: idle.into_summary(),
        });
    }
    if inverse {
        for (round, ops) in f.inverse_batches().iter().enumerate() {
            replay
                .apply(ops)
                .map_err(|e| format!("inverse round {round}: {e}"))?;
            p.apply(ops)
                .map_err(|e| format!("inverse round {round} apply: {e}"))?;
            replay
                .verify(&c)
                .map_err(|e| format!("inverse round {round} trace-prefix: {e}"))?;
            p.maintain()?;
            verify(&c, f, Mode::Ivmlite)
                .map_err(|e| e.with_context(&format!("inverse round {round} oracle")))?;
            let snapshot = validate_snapshot(&c, join, VIEW)?;
            checkpoints.push(Checkpoint {
                phase: "inverse".into(),
                round: Some(round),
                snapshot: snapshot.into_summary(),
            });
        }
        let restored = validate_snapshot(&c, join, VIEW)?;
        if restored.retained_signature() != &signature {
            return Err("inverse trace did not restore decoded bootstrap state".into());
        }
        for (&table, expected) in &f.tables {
            let columns = match table {
                Table::Events => "id,g,amount",
                Table::Lhs => "id,k,g",
                Table::Rhs => "id,k,amount",
            };
            let got = c
                .prepare(&format!(
                    "SELECT {columns} FROM {} ORDER BY id",
                    table.name()
                ))?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut want: Vec<_> = expected
                .iter()
                .map(|r| match table {
                    Table::Events => (r.id, r.g.unwrap(), r.amount.unwrap()),
                    Table::Lhs => (r.id, r.k.unwrap(), r.g.unwrap()),
                    Table::Rhs => (r.id, r.k.unwrap(), r.amount.unwrap()),
                })
                .collect();
            want.sort();
            if got != want {
                return Err(format!("inverse trace did not restore {}", table.name()).into());
            }
        }
    }
    Ok(Correctness {
        checkpoints,
        rows_changed,
    })
}

/// Independent live replay for operation/locality/fanout reporting, outside timing.
/// `sqlite_changes` holds the per-operation row counts SQLite reported (see `Correctness`); they
/// are what `rows_changed` records, and the replayed table state must agree with them.
pub fn target_statistics(f: &Fixture, sqlite_changes: &[Vec<u64>]) -> Result<Vec<TargetStats>> {
    if sqlite_changes.len() != f.batches.len() {
        return Err("SQLite row counts do not cover every round".into());
    }
    let mut live: BTreeMap<Table, BTreeMap<i64, _>> = f
        .tables
        .iter()
        .map(|(&t, rows)| (t, rows.iter().map(|r| (r.id, r.clone())).collect()))
        .collect();
    let mut by_key: BTreeMap<Table, BTreeMap<i64, BTreeSet<i64>>> = BTreeMap::new();
    for (&table, rows) in &live {
        for row in rows.values() {
            if let Some(key) = row.k {
                by_key
                    .entry(table)
                    .or_default()
                    .entry(key)
                    .or_default()
                    .insert(row.id);
            }
        }
    }
    let mut all = Vec::new();
    let hot = (f.cell.group_cardinality as f64 * f.config.hot_group_fraction).floor() as i64;
    for (round, ops) in f.batches.iter().enumerate() {
        let mut stats = TargetStats {
            round,
            operations: ops.len(),
            target_selections: 0,
            hot_anchor_selections: 0,
            distinct_primary_keys: 0,
            repeated_primary_key_touches: 0,
            repeated_existing_targets: 0,
            target_groups: BTreeMap::new(),
            rhs_matched_lhs_rows: 0,
            rhs_matched_hot_lhs_rows: 0,
            rhs_affected_group_visits: 0,
            rhs_hot_group_visits: 0,
            matched_partners_min: usize::MAX,
            matched_partners_max: 0,
            matched_partners_total: 0,
            per_operation: Vec::new(),
        };
        if sqlite_changes[round].len() != ops.len() {
            return Err(
                format!("round {round}: SQLite row counts do not cover every operation").into(),
            );
        }
        let mut keys = BTreeSet::new();
        let mut existing_keys = BTreeSet::new();
        for (index, op) in ops.iter().enumerate() {
            let row = op
                .before
                .as_ref()
                .or(op.after.as_ref())
                .ok_or("empty mutation")?;
            let actual = live.get(&op.table).and_then(|r| r.get(&row.id));
            if op.before.as_ref() != actual {
                return Err("target replay before image differs".into());
            }
            if !keys.insert((op.table, row.id)) {
                stats.repeated_primary_key_touches += 1;
            }
            if let Some(before) = &op.before {
                stats.target_selections += 1;
                if let Some(g) = op.anchor_group {
                    *stats.target_groups.entry(g).or_default() += 1;
                    if g < hot {
                        stats.hot_anchor_selections += 1;
                    }
                }
                if !existing_keys.insert((op.table, before.id)) {
                    stats.repeated_existing_targets += 1;
                }
            }
            let ids = |table| {
                row.k
                    .and_then(|key| by_key.get(&table).and_then(|m| m.get(&key)))
            };
            let mut hot_rows = 0;
            let mut group_count = 0;
            let mut hot_groups = 0;
            let partners = match op.table {
                Table::Events => 1,
                Table::Lhs => ids(Table::Rhs).map_or(0, BTreeSet::len),
                Table::Rhs => {
                    let matches: Vec<_> = ids(Table::Lhs)
                        .into_iter()
                        .flatten()
                        .map(|id| &live[&Table::Lhs][id])
                        .collect();
                    let groups: BTreeSet<_> = matches.iter().map(|r| r.g.unwrap()).collect();
                    hot_rows = matches.iter().filter(|r| r.g.unwrap() < hot).count();
                    group_count = groups.len();
                    hot_groups = groups.iter().filter(|&&g| g < hot).count();
                    stats.rhs_matched_lhs_rows += matches.len();
                    stats.rhs_matched_hot_lhs_rows += hot_rows;
                    stats.rhs_affected_group_visits += group_count;
                    stats.rhs_hot_group_visits += hot_groups;
                    matches.len()
                }
            };
            stats.matched_partners_min = stats.matched_partners_min.min(partners);
            stats.matched_partners_max = stats.matched_partners_max.max(partners);
            stats.matched_partners_total += partners;
            let table_rows = live.get_mut(&op.table).unwrap();
            let mut replayed_changes = 0;
            if let Some(r) = &op.before {
                if table_rows.remove(&r.id).is_some() {
                    replayed_changes = 1;
                }
                if let Some(key) = r.k {
                    by_key
                        .get_mut(&op.table)
                        .unwrap()
                        .get_mut(&key)
                        .unwrap()
                        .remove(&r.id);
                }
            }
            if let Some(r) = &op.after {
                if table_rows.insert(r.id, r.clone()).is_none() && op.before.is_none() {
                    replayed_changes = 1;
                }
                if let Some(key) = r.k {
                    by_key
                        .entry(op.table)
                        .or_default()
                        .entry(key)
                        .or_default()
                        .insert(r.id);
                }
            }
            let rows_changed = sqlite_changes[round][index];
            if rows_changed != replayed_changes {
                return Err(format!(
                    "round {round} operation {index}: SQLite changed {rows_changed} rows, the replayed table state changed {replayed_changes}"
                )
                .into());
            }
            stats.per_operation.push(OperationStats {
                index,
                table: op.table,
                primary_key: row.id,
                kind: format!("{:?}", op.kind()?),
                anchor_group: op.anchor_group,
                matched_partners: partners,
                matched_hot_lhs_rows: hot_rows,
                matched_distinct_groups: group_count,
                matched_hot_groups: hot_groups,
                rows_changed,
            });
        }
        stats.distinct_primary_keys = keys.len();
        all.push(stats);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_workload::m2a::{M2aConfig, Profile};
    fn tiny(join: bool) -> Fixture {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m2a.toml");
        let mut cfg = M2aConfig::load(&path).unwrap();
        cfg.single_rows = 800;
        cfg.single_groups = 20;
        cfg.join_rows = 800;
        cfg.join_groups = 20;
        cfg.fanouts = vec![1, 8];
        cfg.batch_sizes = vec![10];
        let cell = cfg
            .cells()
            .unwrap()
            .into_iter()
            .find(|c| {
                c.shape == if join { Shape::Join } else { Shape::Single }
                    && c.profile == Profile::ZipfHot
            })
            .unwrap();
        cfg.generate(&cell, 42).unwrap()
    }
    fn library() -> std::path::PathBuf {
        ivmlite_test::extension_library()
    }
    #[test]
    fn every_mode_replays_tiny_single_and_join_with_full_results() {
        for join in [false, true] {
            let f = tiny(join);
            correctness(&f, &library(), true).unwrap();
            for mode in MODES {
                let m = time_fixture(&f, mode, &library()).unwrap();
                assert_eq!(m.measured.len(), 5);
                assert_eq!(m.summary.total_ms.is_none(), mode == Mode::NoMaintenance);
            }
        }
    }
    #[test]
    fn totals_are_paired_round_sums_not_component_medians() {
        let r = vec![
            RoundTiming::new(1, 1., Some(100.), Some(1.)),
            RoundTiming::new(2, 2., Some(2.), Some(2.)),
            RoundTiming::new(3, 100., Some(1.), Some(100.)),
        ];
        let s = steady_state(&r).unwrap();
        assert_eq!(s.apply_plus_maintain_ms, Some(101.));
        assert_eq!(s.total_ms, Some(102.));
        assert_ne!(
            s.total_ms,
            Some(s.apply_ms + s.maintain_ms.unwrap() + s.read_ms.unwrap())
        );
    }
    #[test]
    fn actual_apply_path_detects_expired_statements() {
        let f = tiny(false);
        let c = open(Mode::NoMaintenance, &library()).unwrap();
        seed(&c, &f).unwrap();
        let mut p = Prepared::new(&c, &f, Mode::NoMaintenance).unwrap();
        c.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
        assert!(p
            .apply(&f.batches[0])
            .unwrap_err()
            .to_string()
            .contains("reprepare"));
    }
    #[test]
    fn actual_maintenance_paths_detect_expired_statements() {
        for mode in [Mode::Recompute, Mode::CoveringRecompute, Mode::Ivmlite] {
            let f = tiny(false);
            let c = open(mode, &library()).unwrap();
            seed(&c, &f).unwrap();
            bootstrap(&c, &f, mode).unwrap();
            let mut p = Prepared::new(&c, &f, mode).unwrap();
            p.apply(&f.batches[0]).unwrap();
            c.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
            assert!(p.maintain().unwrap_err().to_string().contains("reprepare"));
        }
    }
    #[test]
    fn actual_read_path_detects_expired_statements() {
        let f = tiny(false);
        let c = open(Mode::Recompute, &library()).unwrap();
        seed(&c, &f).unwrap();
        bootstrap(&c, &f, Mode::Recompute).unwrap();
        let mut p = Prepared::new(&c, &f, Mode::Recompute).unwrap();
        c.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
        assert!(p.read().unwrap_err().to_string().contains("reprepare"));
    }
    #[test]
    fn targets_report_real_hot_anchor_and_join_affected_groups() {
        let f = tiny(true);
        let s = target_statistics(&f, &sqlite_changes(&f)).unwrap();
        assert_eq!(s.len(), 6);
        assert!(s.iter().all(|r| r.target_selections == 8));
        assert!(s.iter().all(|r| r.rhs_matched_lhs_rows > 0));
        assert!(s
            .iter()
            .all(|r| r.matched_partners_min == f.cell.fanout
                && r.matched_partners_max == f.cell.fanout));
    }
    #[test]
    fn ordinary_recompute_plan_uses_a_join_key_index() {
        let f = tiny(true);
        let c = open(Mode::Recompute, &library()).unwrap();
        seed(&c, &f).unwrap();
        let plan = plan_text(&query_plan(&c, f.cell.view_sql()).unwrap());
        assert!(
            plan.contains("USING INDEX lhs_k") || plan.contains("USING INDEX rhs_k"),
            "{plan}"
        );
        assert_ordinary_recompute_plan(Shape::Join, &plan).unwrap();
        c.execute_batch("DROP INDEX lhs_k; DROP INDEX rhs_k;")
            .unwrap();
        let unindexed = plan_text(&query_plan(&c, f.cell.view_sql()).unwrap());
        let error = assert_ordinary_recompute_plan(Shape::Join, &unindexed).unwrap_err();
        assert!(error.to_string().contains("join key index"), "{error}");
    }
    #[test]
    fn dml_plans_use_rowid_lookups() {
        for join in [false, true] {
            let f = tiny(join);
            let c = open(Mode::Recompute, &library()).unwrap();
            seed(&c, &f).unwrap();
            let plans = dml_plans(&c, f.tables.keys().copied()).unwrap();
            assert_eq!(plans.len(), 2 * f.tables.len());
            assert_dml_plans(&plans).unwrap();
        }
        let f = tiny(false);
        let c = open(Mode::Recompute, &library()).unwrap();
        seed(&c, &f).unwrap();
        let rewritten =
            plan_text(&query_plan(&c, "UPDATE events SET g=?2 WHERE amount=?1").unwrap());
        let plans = BTreeMap::from([("events.update".to_owned(), rewritten)]);
        let error = assert_dml_plans(&plans).unwrap_err();
        assert!(error.to_string().contains("events.update"), "{error}");
    }
    #[test]
    fn dml_plans_are_unchanged_by_the_view_triggers() {
        let f = tiny(true);
        let plain = open(Mode::Recompute, &library()).unwrap();
        seed(&plain, &f).unwrap();
        let ivm = open(Mode::Ivmlite, &library()).unwrap();
        seed(&ivm, &f).unwrap();
        bootstrap(&ivm, &f, Mode::Ivmlite).unwrap();
        let tables = || f.tables.keys().copied();
        let live = dml_plans(&ivm, tables()).unwrap();
        assert_dml_plans(&live).unwrap();
        assert_eq!(live, dml_plans(&plain, tables()).unwrap());
    }
    #[test]
    fn manifest_pragmas_are_read_back_from_the_connection() {
        let f = tiny(false);
        let probe = probe_provenance(&f).unwrap();
        for name in [
            "foreign_keys",
            "recursive_triggers",
            "journal_mode",
            "synchronous",
            "page_size",
            "cache_size",
            "temp_store",
        ] {
            assert!(probe.pragmas.contains_key(name), "{name}");
        }
        let c = open(Mode::Recompute, &library()).unwrap();
        assert_eq!(read_pragmas(&c).unwrap(), probe.pragmas);
        c.execute_batch("PRAGMA cache_size=-1000").unwrap();
        let changed = read_pragmas(&c).unwrap();
        assert_eq!(changed["cache_size"], "-1000");
        assert_ne!(changed, probe.pragmas);
        let error = check_pragmas(&changed).unwrap_err();
        assert!(error.to_string().contains("cache_size"), "{error}");
    }
    #[test]
    fn mismatch_writes_the_full_symmetric_difference() {
        let (expected_only, actual_only) =
            symmetric_difference(&[(1, 1, 5), (2, 1, 3)], &[(1, 1, 5), (3, 1, 3), (3, 1, 3)]);
        assert_eq!(expected_only, vec![(2, 1, 3)]);
        assert_eq!(actual_only, vec![(3, 1, 3), (3, 1, 3)]);
    }
    #[test]
    fn oracle_check_reports_the_differing_rows_of_a_real_comparison() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE a(x INTEGER, y INTEGER); CREATE TABLE b(x INTEGER, y INTEGER);
             INSERT INTO a VALUES(1,5),(2,3); INSERT INTO b VALUES(1,5),(3,3),(3,3);",
        )
        .unwrap();
        let mismatch = oracle_check(&c, "SELECT * FROM a", "SELECT * FROM b").unwrap_err();
        assert_eq!(mismatch.expected_only, vec![vec![Some(3), Some(3)]; 2]);
        assert_eq!(mismatch.actual_only, vec![vec![Some(2), Some(3)]]);
        assert!(oracle_check(&c, "SELECT * FROM a", "SELECT * FROM a").is_ok());
    }
    fn sqlite_changes(f: &Fixture) -> Vec<Vec<u64>> {
        correctness(f, &library(), false).unwrap().rows_changed
    }
    #[test]
    fn apply_counting_records_the_count_sqlite_reports_even_when_it_is_wrong() {
        let f = tiny(false);
        let c = open(Mode::NoMaintenance, &library()).unwrap();
        seed(&c, &f).unwrap();
        let mut p = Prepared::new(&c, &f, Mode::NoMaintenance).unwrap();
        // Matches every row of the update's group instead of one primary key.
        p.rows.get_mut(&Table::Events).unwrap().update = c
            .prepare("UPDATE events SET g=g+0*?2+0*?3 WHERE g=(SELECT g FROM events WHERE id=?1)")
            .unwrap();
        let op = f.batches[0]
            .iter()
            .find(|op| op.before.is_some() && op.after.is_some())
            .unwrap();
        let mut changes = Vec::new();
        let error = p
            .apply_counting(std::slice::from_ref(op), &mut changes)
            .unwrap_err();
        assert!(changes[0] > 1, "{changes:?}");
        assert!(error.to_string().contains("changed"), "{error}");
    }
    #[test]
    fn targets_record_rows_changed_per_operation() {
        for join in [false, true] {
            let f = tiny(join);
            let changes = sqlite_changes(&f);
            let stats = target_statistics(&f, &changes).unwrap();
            let recorded: Vec<Vec<u64>> = stats
                .iter()
                .map(|r| r.per_operation.iter().map(|op| op.rows_changed).collect())
                .collect();
            assert_eq!(recorded, changes);
            assert!(recorded.iter().flatten().all(|&n| n == 1));
            let mut wrong = changes.clone();
            wrong[0][0] = 2;
            let error = target_statistics(&f, &wrong).unwrap_err();
            assert!(error.to_string().contains("SQLite changed 2"), "{error}");
        }
    }
    #[test]
    fn the_join_key_index_must_be_named_exactly() {
        let near_miss = "SCAN lhs\nSEARCH rhs USING INDEX rhs_k_amount (k=?)";
        assert!(assert_ordinary_recompute_plan(Shape::Join, near_miss).is_err());
        let covering = "SCAN lhs\nSEARCH rhs USING COVERING INDEX rhs_k (k=?)";
        assert_ordinary_recompute_plan(Shape::Join, covering).unwrap();
    }
}
