use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate_database, gen_initial, gen_ops, recompute_via_sqlite,
    view_query_to_sql, Domain, Engine, Op, ViewQuery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Batching {
    /// Apply every delta at once
    All,
    /// Apply each delta on its own
    One,
    /// One batch per n deltas
    Chunks(usize),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestCase {
    pub seed: u64,
    pub database: Database,
    pub query: ViewQuery,
    pub initial: BTreeMap<String, Vec<Row>>,
    pub ops: Vec<(String, Op)>,
    pub batching: Batching,
}

impl TestCase {
    /// This case's batches, produced by the same code path `run` uses internally.
    pub fn batches(&self) -> Vec<BTreeMap<String, Vec<(Row, i64)>>> {
        batches(&self.ops, self.batching)
    }
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub case_seed: u64,
    pub stage: String,
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[seed={seed}] {stage}: {detail}\n\
             replay this case: IVMLITE_SEED={seed} cargo test -p ivmlite-test --test harness_catches_bugs -- --nocapture",
            seed = self.case_seed,
            stage = self.stage,
            detail = self.detail
        )
    }
}

fn parse_seed_arg(raw: Option<String>) -> Vec<u64> {
    match raw {
        Some(s) => match s.parse::<u64>() {
            Ok(seed) => vec![seed],
            Err(_) => panic!("IVMLITE_SEED must be a u64, got {s:?}"),
        },
        None => (0..50).collect(),
    }
}

/// The range of seeds the integration tests walk. With `IVMLITE_SEED` set, only
/// that one seed runs — the mechanism behind the replay command a `Failure`
/// prints (spec §9.4).
pub fn seed_range() -> Vec<u64> {
    parse_seed_arg(std::env::var("IVMLITE_SEED").ok())
}

/// Generate a differential test case. `db` declares every base table the case
/// involves (in a deterministic order, spec §9.4); the query is picked from
/// `enumerate_database(db)` by `seed % len`, which alternates the anchor's
/// single-table queries with — for two or more tables — the join queries over
/// the first two (M1b Phase 2a, Ruling 5).
pub fn gen_case(
    seed: u64,
    db: &Database,
    domain: &Domain,
    rows_per_table: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    assert!(!db.is_empty(), "the database should not be empty");
    let queries = enumerate_database(db);
    let query = queries[seed as usize % queries.len()].clone();
    gen_case_with_query(seed, db, domain, query, rows_per_table, op_count, batching)
}

/// The same code path as `gen_case`, but the caller supplies the `query` instead
/// of one being picked from `enumerate` by seed — covering the query space
/// **one query at a time** through the enumeration needs this entry point.
pub fn gen_case_with_query(
    seed: u64,
    db: &Database,
    domain: &Domain,
    query: ViewQuery,
    rows_per_table: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    let mut rng = StdRng::seed_from_u64(seed);
    let initial = gen_initial(&mut rng, db, domain, rows_per_table);
    let ops = gen_ops(&mut rng, db, domain, &initial, op_count);
    TestCase {
        seed,
        database: db.clone(),
        query,
        initial,
        ops,
        batching,
    }
}

/// Split the ops into batches, grouping each batch by table into
/// **unconsolidated** raw `(Row, i64)` sequences — the same row may appear
/// several times in one batch and table, and whether to consolidate is left to
/// the engine's `apply` (spec §8.2). The harness folds nothing itself, which is
/// exactly why M1's consolidation has to really land for the tests to pass.
/// Grouping through a `BTreeMap` makes the per-table iteration order
/// deterministic (spec §9.4); within a group the original op order is kept.
fn batches(ops: &[(String, Op)], batching: Batching) -> Vec<BTreeMap<String, Vec<(Row, i64)>>> {
    let size = match batching {
        Batching::All => ops.len().max(1),
        Batching::One => 1,
        Batching::Chunks(n) => n.max(1),
    };
    ops.chunks(size)
        .map(|chunk| {
            let mut grouped: BTreeMap<String, Vec<(Row, i64)>> = BTreeMap::new();
            for (table, op) in chunk {
                grouped
                    .entry(table.clone())
                    .or_default()
                    .extend(op.to_delta());
            }
            grouped
        })
        .collect()
}

fn initial_bases(initial: &BTreeMap<String, Vec<Row>>) -> BTreeMap<String, ZSet> {
    initial
        .iter()
        .map(|(table, rows)| {
            (
                table.clone(),
                ZSet::from_rows(rows.iter().cloned().map(|r| (r, 1))),
            )
        })
        .collect()
}

/// Run one case: apply the deltas batch by batch, and at **every observable
/// refresh point** check the invariants and compare strictly against the oracle.
///
/// Why comparing only the final state is not enough (spec §9.1): an
/// implementation that "goes wrong midway, stays well-formed, and later
/// recovers on its own" passes an end-only comparison entirely — and that is
/// the typical shape of a state-drift bug. The invariant layer cannot stop it,
/// because wrong values also satisfy "weight 1, unique group key".
///
/// The cost is complexity going from O(n) to O(n × base-table size), so
/// differential test cases must stay small (25 rows of initial data and 150
/// operations by default). Large-scale scenarios are left to the benchmark.
///
/// Each batch calls `refresh` once, not once per table (spec §8.2, "N applies,
/// one refresh"): the batch's `(table name, Op)` pairs are grouped by table,
/// `apply` is called once for each table with changes, and then `refresh` once —
/// the only place consolidation can take effect.
pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure> {
    let fail = |stage: &str, detail: String| Failure {
        case_seed: case.seed,
        stage: stage.to_string(),
        detail,
    };

    let compare = |engine: &mut E,
                   bases: &BTreeMap<String, ZSet>,
                   stage: &str|
     -> Result<(), Failure> {
        let got = engine
            .materialize()
            .map_err(|e| fail(&format!("materialize[{stage}]"), e.to_string()))?;
        check_invariants(&got, &case.query)
            .map_err(|e| fail(&format!("invariants[{stage}]"), e))?;
        let want = recompute_via_sqlite(&case.database, &case.query, bases)
            .map_err(|e| fail(&format!("oracle[{stage}]"), e.to_string()))?;
        if got != want {
            return Err(fail(
                    &format!("diff[{stage}]"),
                    format!(
                        "the engine disagrees with the oracle\n  query: {}\n  engine: {:?}\n  oracle: {:?}",
                        view_query_to_sql(&case.query, &case.database),
                        got,
                        want
                    ),
                ));
        }
        Ok(())
    };

    let mut bases = initial_bases(&case.initial);
    engine
        .create_view(&case.database, &case.query, &bases)
        .map_err(|e| fail("create_view", e.to_string()))?;

    // Compare once right after bootstrap — so a case with no ops is genuinely checked too.
    compare(engine, &bases, "bootstrap")?;

    for (i, grouped) in case.batches().into_iter().enumerate() {
        for (table, raw) in &grouped {
            engine
                .apply(table, raw)
                .map_err(|e| fail(&format!("apply[{i}][{table}]"), e.to_string()))?;
        }
        engine
            .refresh()
            .map_err(|e| fail(&format!("refresh[{i}]"), e.to_string()))?;
        // The harness's own reference bookkeeping merges here — that is the
        // harness's business, not the engine's (spec §8.2). The engine still sees
        // each table's `raw` in its original form.
        for (table, raw) in &grouped {
            let zset = bases.entry(table.clone()).or_default();
            for (row, weight) in raw {
                zset.update(row.clone(), *weight);
            }
        }
        compare(engine, &bases, &i.to_string())?;
    }
    Ok(())
}

/// The second layer of spec §9.1: however the same delta sequence is batched,
/// the final state must be the same. This property cannot be tested under an
/// automatic-maintenance mode — one of the benefits of v0 choosing explicit
/// refresh.
pub fn check_batch_invariance<E, F>(case: &TestCase, make: F) -> Result<(), Failure>
where
    E: Engine,
    F: Fn() -> E,
{
    let modes = [
        Batching::All,
        Batching::One,
        Batching::Chunks(3),
        Batching::Chunks(17),
    ];
    let mut reference: Option<(Batching, ZSet)> = None;

    for mode in modes {
        let mut engine = make();
        let scoped = TestCase {
            batching: mode,
            ..case.clone()
        };
        run(&mut engine, &scoped)?;
        let state = engine.materialize().map_err(|e| Failure {
            case_seed: case.seed,
            stage: format!("batch_invariance[{mode:?}]"),
            detail: e.to_string(),
        })?;

        match &reference {
            None => reference = Some((mode, state)),
            Some((ref_mode, ref_state)) => {
                if *ref_state != state {
                    return Err(Failure {
                        case_seed: case.seed,
                        stage: "batch_invariance".into(),
                        detail: format!(
                            "{ref_mode:?} and {mode:?} reach different final states\n  {ref_state:?}\n  {state:?}"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        enumerate, enumerate_join, gen_database, gen_database_with_swapped_right_table, Agg, AggFn,
        Column, ColumnType, Domain, EngineError, NaiveRecompute, Predicate, Schema,
    };
    use ivmlite_core::Value;

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
            ],
        }
    }

    // The same concept as the single-table wrappers in naive.rs / buggy.rs /
    // ops.rs / oracle.rs, but this one builds a whole TestCase (query included),
    // a different shape, so it was not merged into test_support —
    // `single_table_db` is the shared half (m4).
    fn single_table_case(seed: u64, rows: Vec<Row>, ops: Vec<Op>, batching: Batching) -> TestCase {
        let schema = schema();
        let db = crate::test_support::single_table_db(&schema);
        let query = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
            join: None,
        };
        let initial = crate::test_support::as_initial(&schema, rows);
        let ops = ops
            .into_iter()
            .map(|op| (schema.table.clone(), op))
            .collect();
        TestCase {
            seed,
            database: db,
            query,
            initial,
            ops,
            batching,
        }
    }

    /// Records and does nothing else: the real computation is delegated to
    /// `NaiveRecompute` (so `run`'s internal oracle comparison cannot fail
    /// because of an error in our own engine), while every `(table, raw)`
    /// `apply` receives is stored verbatim for the guard tests to assert on.
    #[derive(Debug, Default)]
    struct RecordingEngine {
        inner: NaiveRecompute,
        received: Vec<(String, Vec<(Row, i64)>)>,
    }

    impl Engine for RecordingEngine {
        fn create_view(
            &mut self,
            db: &Database,
            query: &ViewQuery,
            initial: &BTreeMap<String, ZSet>,
        ) -> Result<(), EngineError> {
            self.inner.create_view(db, query, initial)
        }

        fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
            self.received.push((table.to_string(), raw.to_vec()));
            self.inner.apply(table, raw)
        }

        fn refresh(&mut self) -> Result<(), EngineError> {
            self.inner.refresh()
        }

        fn materialize(&mut self) -> Result<ZSet, EngineError> {
            self.inner.materialize()
        }
    }

    /// Guard 1: when the same row appears twice in one batch, the engine must
    /// receive two `(row, +1)` entries as they are, not one `(row, +2)` merged
    /// by the harness.
    ///
    /// How to break it: make `batches()` (or the step in `run` that passes to
    /// `apply`) fold the chunk into a `ZSet` and expand it again — this test must
    /// go red.
    #[test]
    fn apply_receives_unconsolidated_raw_deltas() {
        let dup = Row::new(vec![Value::Text("a".into()), Value::Int(1)]);
        let case = single_table_case(
            0,
            vec![],
            vec![Op::Insert(dup.clone()), Op::Insert(dup.clone())],
            Batching::All,
        );

        let mut engine = RecordingEngine::default();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("should not fail: {f}"));

        assert_eq!(
            engine.received.len(),
            1,
            "with Batching::All the two ops should land in the same batch"
        );
        let (_, raw) = &engine.received[0];
        let dup_entries = raw.iter().filter(|(row, w)| *row == dup && *w == 1).count();
        assert_eq!(
            dup_entries, 2,
            "a row inserted twice must reach the engine as two separate (row, +1) entries, not one merged entry"
        );
    }

    /// Guard 2: the table name must reach the engine as it is — the precondition
    /// for join, which reads several base tables.
    ///
    /// How to break it: in `run`, replace `apply`'s table-name argument with a
    /// hard-coded constant or an empty string — this test must go red.
    #[test]
    fn apply_receives_the_schema_table_name() {
        let case = single_table_case(
            0,
            vec![],
            vec![Op::Insert(Row::new(vec![
                Value::Text("a".into()),
                Value::Int(1),
            ]))],
            Batching::All,
        );

        let mut engine = RecordingEngine::default();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("should not fail: {f}"));

        assert_eq!(engine.received.len(), 1);
        assert_eq!(
            engine.received[0].0,
            case.database.tables()[0].table,
            "the table name apply receives must equal the one the case declares"
        );
    }

    /// Guard 3: `refresh` is load-bearing — after `apply` without a `refresh`,
    /// `materialize` must still return the pre-apply state, changing only once
    /// `refresh` is called.
    ///
    /// How to break it: make `NaiveRecompute::apply` merge straight into `base`
    /// (M0's behaviour) — this test must go red, since `refresh` would then be a
    /// no-op with no observable effect.
    #[test]
    fn refresh_is_load_bearing_for_naive_recompute() {
        let schema = schema();
        let query = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
            join: None,
        };
        let db = Database::single(schema.clone());
        let bases = BTreeMap::from([(
            schema.table.clone(),
            ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Int(1)]), 1)]),
        )]);
        let mut engine = NaiveRecompute::new();
        engine.create_view(&db, &query, &bases).unwrap();
        let before = engine.materialize().unwrap();

        let new_row = Row::new(vec![Value::Text("b".into()), Value::Int(2)]);
        engine.apply("orders", &[(new_row, 1)]).unwrap();

        let still_before = engine.materialize().unwrap();
        assert_eq!(
            still_before, before,
            "after apply and before refresh, materialize must still show the pre-apply state"
        );

        engine.refresh().unwrap();
        let after = engine.materialize().unwrap();
        assert_ne!(
            after, before,
            "after refresh, materialize must reflect the changes just applied"
        );
    }

    /// I3's recording engine: besides recording what it received, it splits the
    /// `apply` calls into batches at each `refresh` — a sequence of
    /// `(table, row_count)`, grouped by batch. `NaiveRecompute` still does the
    /// real computation; this only adds bookkeeping.
    #[derive(Debug, Default)]
    struct OrderRecordingEngine {
        inner: NaiveRecompute,
        batches: Vec<Vec<(String, usize)>>,
        current_batch: Vec<(String, usize)>,
    }

    impl Engine for OrderRecordingEngine {
        fn create_view(
            &mut self,
            db: &Database,
            query: &ViewQuery,
            initial: &BTreeMap<String, ZSet>,
        ) -> Result<(), EngineError> {
            self.inner.create_view(db, query, initial)
        }

        fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
            self.current_batch.push((table.to_string(), raw.len()));
            self.inner.apply(table, raw)
        }

        fn refresh(&mut self) -> Result<(), EngineError> {
            self.inner.refresh()?;
            self.batches.push(std::mem::take(&mut self.current_batch));
            Ok(())
        }

        fn materialize(&mut self) -> Result<ZSet, EngineError> {
            self.inner.materialize()
        }
    }

    /// I3: pins the **only** observable consequence of `batches()` grouping
    /// through a `BTreeMap` — within one batch, the table names `apply` receives
    /// in turn must strictly follow `db.tables()`'s order. (For the
    /// `t0..t{n-1}` names `gen_database` produces, insertion order and
    /// lexicographic order happen to coincide, so the assertion is equivalent to
    /// "lexicographic"; but writing it against `db.tables()`'s order matches
    /// I3's concern better: once §7.2/§7.3 land this order feeds GC and the
    /// bootstrap watermark, and what must then hold is "consistent with
    /// `db.tables()`", not the coincidence "lexicographic order happens to be
    /// right".)
    ///
    /// This guard is **absolute, not statistical** as long as `batches()` keeps
    /// grouping through a `BTreeMap`: the `Vec`'s iteration order is pinned on
    /// the `ivmlite-core` side by `table_order_is_preserved`, and a `BTreeMap`
    /// iterating in key order is behaviour the standard library documents,
    /// independent of any random state or runtime environment — the same input
    /// gives the same order on every run, with no "got lucky this time".
    ///
    /// Mutation check (see I3's mutation record): with `batches()`'s return
    /// type and internal grouping container both changed to `HashMap`, this
    /// guard goes red — but red in that direction is **statistical**: a
    /// `HashMap`'s iteration order comes from a `RandomState` generated on every
    /// construction, the fewer the keys the likelier a coincidentally correct
    /// order, and a run that happens to get the right order cannot be ruled out
    /// in theory. What this test pins as an absolute guarantee is only the
    /// "keep using a `BTreeMap`" side.
    #[test]
    fn per_batch_apply_order_follows_db_tables_order() {
        let db = gen_database(4);
        let domain = Domain::default();
        let case = gen_case(21, &db, &domain, 20, 150, Batching::Chunks(5));

        let mut engine = OrderRecordingEngine::default();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("should not fail: {f}"));

        let table_order: Vec<String> = db.tables().iter().map(|t| t.table.clone()).collect();

        let mut multi_table_batches = 0usize;
        for batch in &engine.batches {
            if batch.len() > 1 {
                multi_table_batches += 1;
            }
            let mut last_idx: Option<usize> = None;
            for (table, _row_count) in batch {
                let idx = table_order
                    .iter()
                    .position(|t| t == table)
                    .unwrap_or_else(|| panic!("unknown table {table}"));
                if let Some(last) = last_idx {
                    assert!(
                        idx > last,
                        "within a batch, table names must strictly follow db.tables(): in \
                         {table_order:?} the previous table's index was {last}, this one is \
                         {idx} (table {table})"
                    );
                }
                last_idx = Some(idx);
            }
        }
        assert!(
            multi_table_batches > 0,
            "at least one batch must really involve several tables, or the order assertion above never runs on a meaningful path"
        );
    }

    #[test]
    fn naive_engine_passes_every_enumerated_query() {
        let schema = schema();
        let db = Database::single(schema.clone());
        let domain = Domain::default();
        for (i, query) in crate::enumerate(&schema).into_iter().enumerate() {
            let mut case = gen_case(i as u64, &db, &domain, 30, 200, Batching::Chunks(7));
            case.query = query;
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} failed at {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn batch_invariance_holds_for_naive_engine() {
        let db = Database::single(schema());
        let domain = Domain::default();
        // An explicit stride into `enumerate`'s space, not `gen_case`'s plain
        // `seed % len` (M1b Phase 2a final review, Important 2): taken
        // straight, seed 4242 now lands on a COUNT-only, one-column-group-by
        // query, silently dropping this test's SUM / two-column-group-by
        // coverage. The assertions below keep that from happening quietly
        // again.
        const BATCH_INVARIANCE_QUERY_STRIDE: usize = 11;
        let seed = 4242u64;
        let queries = enumerate(&schema());
        let query =
            queries[(seed as usize * BATCH_INVARIANCE_QUERY_STRIDE) % queries.len()].clone();
        assert!(
            query.aggs.iter().any(|a| a.func == AggFn::Sum),
            "this case must exercise SUM: {query:?}"
        );
        assert!(
            query.group_by.len() == 2,
            "this case must exercise a two-column group-by: {query:?}"
        );
        let case = gen_case_with_query(seed, &db, &domain, query, 30, 200, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    /// I4: `check_batch_invariance` had never run on a genuinely multi-table case
    /// — the review's probe `assert!(case.database.len() <= 1)` confirmed it.
    /// This uses a two-table `Database`, so `run` (which
    /// `check_batch_invariance` calls once per `Batching`) really goes through
    /// multi-table apply routing and the harness-side `bases` bookkeeping.
    ///
    /// With a join query, the non-anchor table's deltas reach both
    /// `materialize()` and the oracle. Measured (item 3 of the join-landing
    /// record in `docs/mutation-gates.md`): `NaiveRecompute::apply` dropping
    /// non-anchor deltas, or `run`'s `bases` skipping non-anchor tables, each
    /// reddens this test at `diff[0]` — through `run`'s per-batch oracle
    /// comparison, before the batch-independence comparison is reached. With
    /// both mutations at once it stays green: the engine and the oracle then
    /// read the same stale right table.
    #[test]
    fn batch_invariance_holds_for_naive_engine_on_a_two_table_case() {
        let db = gen_database(2);
        let domain = Domain::default();
        let case = gen_case_with_query(
            4343,
            &db,
            &domain,
            join_on_column_0(&db),
            30,
            200,
            Batching::All,
        );
        assert_eq!(
            case.database.len(),
            2,
            "this test must be a genuine two-table case"
        );
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    /// The share of seeds whose initial state gives both tables at least one
    /// common non-NULL key value on column 0.
    const MIN_SEEDS_WITH_INITIAL_KEY_OVERLAP_PERCENT: usize = 90;
    /// The share of batches in which both tables change rows that share a
    /// non-NULL key value on column 0 — the batches that exercise ΔR⋈ΔS.
    const MIN_BATCHES_WITH_SHARED_DELTA_KEY_PERCENT: usize = 5;

    fn keys(rows: &[Row]) -> std::collections::BTreeSet<Value> {
        rows.iter()
            .map(|r| r.get(0).clone())
            .filter(|v| *v != Value::Null)
            .collect()
    }

    fn join_on_column_0(db: &Database) -> ViewQuery {
        enumerate_join(&db.tables()[0], &db.tables()[1])
            .into_iter()
            .find(|q| {
                q.join
                    .as_ref()
                    .is_some_and(|j| j.left_column == 0 && j.right_column == 0)
            })
            .expect("the join space includes the k = k join")
    }

    #[test]
    fn join_keys_overlap_in_the_initial_state() {
        // A join test is only as good as its matches: if the two tables drew
        // their keys from disjoint value sets, every join would be empty and
        // every join test trivially green.
        let db = gen_database(2);
        let seeds = 50;
        let overlapping = (0..seeds)
            .filter(|&seed| {
                let case = gen_case_with_query(
                    seed,
                    &db,
                    &Domain::default(),
                    join_on_column_0(&db),
                    25,
                    0,
                    Batching::All,
                );
                !keys(&case.initial["t0"]).is_disjoint(&keys(&case.initial["t1"]))
            })
            .count();
        assert!(
            overlapping * 100 >= seeds as usize * MIN_SEEDS_WITH_INITIAL_KEY_OVERLAP_PERCENT,
            "only {overlapping} of {seeds} seeds have overlapping join keys"
        );
    }

    #[test]
    fn join_cases_exercise_the_delta_cross_term() {
        // ΔR⋈ΔS is non-empty only when both tables change rows with the same
        // key in the same batch. Without such batches, dropping that term from
        // the join would go unnoticed by the differential harness.
        let db = gen_database(2);
        let (mut shared, mut total) = (0usize, 0usize);
        for seed in 0..50 {
            let case = gen_case_with_query(
                seed,
                &db,
                &Domain::default(),
                join_on_column_0(&db),
                25,
                150,
                Batching::Chunks(5),
            );
            for batch in case.batches() {
                total += 1;
                let side = |t: &str| {
                    let rows: Vec<Row> = batch
                        .get(t)
                        .map_or(Vec::new(), |d| d.iter().map(|(r, _)| r.clone()).collect());
                    keys(&rows)
                };
                if !side("t0").is_disjoint(&side("t1")) {
                    shared += 1;
                }
            }
        }
        assert!(
            shared * 100 >= total * MIN_BATCHES_WITH_SHARED_DELTA_KEY_PERCENT,
            "only {shared} of {total} batches change both tables on a shared key"
        );
    }

    #[test]
    fn gen_case_picks_join_queries_for_two_table_databases() {
        // Checklist items 1 and 2 (non-anchor state reaching the engine and the
        // oracle) are observable only through join queries, and the tests that
        // observe them take their query from `gen_case`.
        let db = gen_database(2);
        let joins = (0..50)
            .filter(|&seed| {
                gen_case(seed, &db, &Domain::default(), 5, 0, Batching::All)
                    .query
                    .join
                    .is_some()
            })
            .count();
        assert!(
            joins >= 20,
            "only {joins} of seeds 0..50 picked a join query"
        );
    }

    /// Every join query over `db`'s first two tables, one case each, seeded by
    /// its index, run against `NaiveRecompute`.
    fn naive_engine_passes_every_join_query_over(db: &Database) {
        let domain = Domain::default();
        for (i, query) in enumerate_join(&db.tables()[0], &db.tables()[1])
            .into_iter()
            .enumerate()
        {
            let case =
                gen_case_with_query(i as u64, db, &domain, query, 20, 60, Batching::Chunks(4));
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} failed at {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn naive_engine_passes_every_enumerated_join_query() {
        naive_engine_passes_every_join_query_over(&gen_database(2));
    }

    /// The same sweep with the right table's columns swapped, so the join keys
    /// sit at different positions — `(0, 1)` and `(1, 0)` — in the two tables.
    /// This catches a key-index mistake in `NaiveRecompute`'s own join, or in
    /// the oracle's `ON` clause, when the mistake sits in only one of the two.
    /// A mistake **shared** by both — they then compute the same wrong join —
    /// cancels out here; only
    /// `incremental_engine_is_green_across_the_join_space_with_keys_at_different_positions`
    /// (see the matching row in `docs/mutation-gates.md`) catches that case.
    #[test]
    fn naive_engine_passes_every_join_query_with_keys_at_different_positions() {
        naive_engine_passes_every_join_query_over(&gen_database_with_swapped_right_table());
    }

    #[test]
    fn same_seed_produces_the_same_case() {
        let db = Database::single(schema());
        let domain = Domain::default();
        let a = gen_case(5, &db, &domain, 10, 40, Batching::One);
        let b = gen_case(5, &db, &domain, 10, 40, Batching::One);
        assert_eq!(a.initial, b.initial);
        assert_eq!(a.ops, b.ops);
    }

    #[test]
    fn parse_seed_arg_defaults_to_fifty_seeds_when_unset() {
        assert_eq!(parse_seed_arg(None), (0..50).collect::<Vec<u64>>());
    }

    #[test]
    fn parse_seed_arg_returns_just_the_one_seed_when_set() {
        assert_eq!(parse_seed_arg(Some("7".to_string())), vec![7]);
    }

    #[test]
    #[should_panic(expected = "IVMLITE_SEED")]
    fn parse_seed_arg_panics_on_non_numeric_value() {
        parse_seed_arg(Some("abc".to_string()));
    }
}
