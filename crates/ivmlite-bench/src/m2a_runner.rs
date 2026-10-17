//! Execution of exported M2a traces. Generation and state inspection are untimed.
use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use ivmlite_test::demo_bench::{check_same_multiset, multiset_difference, sorted_rows, Summary};
use ivmlite_test::open_with_extension_at;
use ivmlite_workload::m2a::{Fixture, Mutation, Shape, Table};
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
#[derive(Debug, Clone, Serialize)]
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
    c.execute_batch("PRAGMA page_size=4096; PRAGMA cache_size=-65536; PRAGMA temp_store=MEMORY; PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON;")?;
    Ok(c)
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
fn indexed_query(f: &Fixture) -> String {
    match f.cell.shape {Shape::Single=>"SELECT g, COUNT(*), SUM(amount) FROM events INDEXED BY events_g_amount GROUP BY g".into(),Shape::Join=>"SELECT lhs.g, COUNT(*), SUM(rhs.amount) FROM lhs INDEXED BY lhs_g_k JOIN rhs INDEXED BY rhs_k_amount ON lhs.k = rhs.k GROUP BY lhs.g".into()}
}
fn query_plan(c: &Connection, query: &str) -> Result<Vec<String>> {
    Ok(c.prepare(&format!("EXPLAIN QUERY PLAN {query}"))?
        .query_map([], |r| r.get::<_, String>(3))?
        .collect::<rusqlite::Result<_>>()?)
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
fn verify(c: &Connection, f: &Fixture, mode: Mode) -> Result<()> {
    if mode != Mode::NoMaintenance {
        let actual = if mode == Mode::Ivmlite {
            format!("SELECT * FROM {VIEW}")
        } else {
            "SELECT * FROM m2a_result".into()
        };
        check_same_multiset(c, &actual, f.cell.view_sql())?;
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
                    delete: c.prepare(&format!("DELETE FROM {} WHERE id=?1", t.name()))?,
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
    let plan = query_plan(&c, &query)?;
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
        check_same_multiset(&c, &query, f.cell.view_sql())?;
    }
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
        check_same_multiset(&c, &query, f.cell.view_sql())?;
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

pub fn correctness(f: &Fixture, library: &Path, inverse: bool) -> Result<Vec<Checkpoint>> {
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
    for (round, ops) in f.batches.iter().enumerate() {
        let old = sorted_rows(&c, &format!("SELECT * FROM {VIEW}"))?;
        replay
            .apply(ops)
            .map_err(|e| format!("forward round {round}: {e}"))?;
        p.apply(ops)
            .map_err(|e| format!("forward round {round} apply: {e}"))?;
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
        verify(&c, f, Mode::Ivmlite).map_err(|e| format!("forward round {round} oracle: {e}"))?;
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
                .map_err(|e| format!("inverse round {round} oracle: {e}"))?;
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
    Ok(checkpoints)
}

/// Independent live replay for operation/locality/fanout reporting, outside timing.
pub fn target_statistics(f: &Fixture) -> Result<Vec<TargetStats>> {
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
            });
            stats.matched_partners_min = stats.matched_partners_min.min(partners);
            stats.matched_partners_max = stats.matched_partners_max.max(partners);
            stats.matched_partners_total += partners;
            if let Some(r) = &op.before {
                live.get_mut(&op.table).unwrap().remove(&r.id);
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
                live.get_mut(&op.table).unwrap().insert(r.id, r.clone());
                if let Some(key) = r.k {
                    by_key
                        .entry(op.table)
                        .or_default()
                        .entry(key)
                        .or_default()
                        .insert(r.id);
                }
            }
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
        let s = target_statistics(&f).unwrap();
        assert_eq!(s.len(), 6);
        assert!(s.iter().all(|r| r.target_selections == 8));
        assert!(s.iter().all(|r| r.rhs_matched_lhs_rows > 0));
        assert!(s
            .iter()
            .all(|r| r.matched_partners_min == f.cell.fanout
                && r.matched_partners_max == f.cell.fanout));
    }
}
