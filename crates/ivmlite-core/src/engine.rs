use std::collections::{BTreeMap, BTreeSet};

use crate::{fresh_mem_arrangement, lower, Database, Node, Row, ViewQuery, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// Wraps `Node` so that "how many rows a call pushed" is bookkeeping the
/// operator tree keeps itself, not a number the caller computes separately
/// before the call.
///
/// Final review Finding B: `refresh` used to compute `rows_processed` on its
/// own, from the pre-push `delta.len()`, with `tree.delta(table, delta)` on the
/// very next line but syntactically unrelated to it. Changing `refresh` to call
/// `tree.delta` once per row of `delta` (instead of once for the whole batch)
/// still compiled and kept the same semantics (the delta was already merged in
/// the `by_table` step), and "consolidation pushes the operator tree fewer
/// times" — the one performance story spec §8.2/§8.5 cares about — became
/// completely untestable through that counter.
///
/// So `pushes` / `rows_fed` are not guessed: they are recorded when
/// `Node::delta` is **actually called**, and however many calls the caller
/// splits one batch into, the two numbers reflect it faithfully.
///
/// But **do not treat the `node` field's privacy as a structural guarantee**:
/// Rust field privacy is module-level, and `CountingTree` and
/// `IncrementalEngine` share this file, so `refresh` could perfectly well write
/// `tree.node.delta(...)` and bypass the counting — the final re-review measured
/// that it compiles. Such a bypass is caught today (the counters stay at 0 and
/// the counter tests go red), but because the existing tests assert exact
/// non-zero values, not because the type prevents it. Revisit this if other
/// code holding a `CountingTree` is ever added to this file.
#[derive(Debug)]
struct CountingTree {
    node: Node,
    /// How many times `Node::delta` (the top-level entry) was called in this
    /// `refresh`. With consolidation in effect, each table should be pushed
    /// exactly once per `refresh`, however many rows remain after merging.
    pushes: usize,
    /// The total rows fed to `Node::delta` in this `refresh` — observed from
    /// the calls' own arguments, not computed separately from the pre-push
    /// `ZSet`.
    rows_fed: usize,
}

impl CountingTree {
    fn new(node: Node) -> Self {
        Self {
            node,
            pushes: 0,
            rows_fed: 0,
        }
    }

    fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        self.pushes += 1;
        self.rows_fed += input.len();
        self.node.delta(table, input)
    }

    fn reset_counts(&mut self) {
        self.pushes = 0;
        self.rows_fed = 0;
    }
}

/// v0's incremental engine.
///
/// `apply` only accumulates pending deltas; `refresh` is what pushes them
/// through the operator tree — spec §8.5 requires the two to be separate, and
/// §8.2 makes explicit refresh a permanent API rather than a temporary v0
/// compromise.
#[derive(Debug, Default)]
pub struct IncrementalEngine {
    tree: Option<CountingTree>,
    /// Every table name `create_view` declared — `apply` uses it to reject
    /// undeclared tables (final review Finding L). The engine does not hold the
    /// `Database` itself, so this is the only record.
    tables: BTreeSet<String>,
    /// The view's current materialized result. The deltas the operators emit are merged into it.
    view: ZSet,
    /// Raw Δ ingested but not yet maintained, **unconsolidated** (§8.5). A
    /// `Vec` rather than a per-table map: arrival order within the batch is kept
    /// until `refresh`, and whether to merge is `refresh`'s decision (Task 6).
    pending: Vec<(String, Row, i64)>,
    /// The rows the last `refresh` actually pushed through the operator tree.
    ///
    /// Consolidation changes the amount of work, not the result, so it cannot
    /// be observed through "is the output right" — the situation spec §8.5
    /// calls structurally invisible. This counter is its only observable
    /// footprint, and the source of §11's "write amplification has a concrete
    /// number".
    ///
    /// Since final review Finding B the number comes from
    /// `CountingTree::rows_fed` — the total rows `Node::delta` received when it
    /// was actually called — not a `delta.len()` that `refresh` computes on its
    /// own before the call.
    rows_processed: usize,
    /// How many times `Node::delta` (the top-level entry) was called in the
    /// last `refresh` — see `CountingTree`'s doc comment.
    tree_pushes: usize,
}

impl IncrementalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        let anchor = db
            .tables()
            .first()
            // In M1b the table order comes from `__ivm_dep`, not from the
            // query's FROM clause — choosing "the first" here is only v0's
            // existing convention (final review Finding I): in single-table
            // cases the anchor happens to coincide with "the table the query
            // reads", but that is no guarantee of `db.tables()`'s order, merely
            // something no more complex case has yet exposed.
            .ok_or_else(|| EngineError("the Database must have at least one table".into()))?;
        let plan = lower(query, anchor).map_err(|e| EngineError(e.0))?;
        let mut tree = CountingTree::new(Node::build(&plan, &mut fresh_mem_arrangement));

        // Bootstrap: push each table's initial state through as the first batch
        // of deltas. A declared table with no initial state is an error, not an
        // empty table — the same convention as the oracle's
        // `missing_base_state_for_a_declared_table_is_an_error`, pinned here
        // directly by `create_view_errors_when_a_declared_table_has_no_initial_state`
        // (M1a Phase 2 Task 5 re-review Finding 2; the differential harness's
        // `gen_initial` always fills every table and never reaches this path,
        // hence a unit test that does not depend on the harness's case
        // distribution).
        //
        // Final review Finding A: the bootstrap loop builds into a local `view`,
        // and only once the whole loop has succeeded are `self.view` /
        // `self.tree` / `self.tables` committed together. Previously
        // `self.view = ZSet::new()` was written into `self` before the loop, so
        // when the loop returned early through `?` for a table with no initial
        // state, `self.view` was left a half-built view that had absorbed only
        // some tables while `self.tree` still held the tree built by the last
        // successful `create_view` — permanently inconsistent, with no later
        // `apply` / `refresh` raising an error, only quietly computing wrong
        // answers.
        let mut view = ZSet::new();
        for schema in db.tables() {
            let base = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!(
                    "table {} is declared but has no initial state",
                    schema.table
                ))
            })?;
            view.merge(&tree.delta(&schema.table, base));
        }
        self.view = view;
        self.tables = db.tables().iter().map(|s| s.table.clone()).collect();
        self.tree = Some(tree);
        self.pending.clear();
        Ok(())
    }

    pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        if self.tree.is_none() {
            return Err(EngineError("apply was called before create_view".into()));
        }
        // Final review Finding L: the engine does not hold the `Database`, and
        // used to accept any table name — piling it into `pending` and feeding
        // it to `Node::Scan` at `refresh`, where `Scan`, routing only by table
        // name (see `node.rs`), quietly swallowed an unknown name as an empty
        // delta. `apply` "succeeded" while the view was not affected at all. In
        // M1b these names come across the shadow-table seam, and a mismatch must
        // be an error here, not a stale-but-plausible view.
        if !self.tables.contains(table) {
            return Err(EngineError(format!(
                "apply received the undeclared table {table}; create_view declared {:?}",
                self.tables
            )));
        }
        self.pending
            .extend(raw.iter().map(|(r, w)| (table.to_string(), r.clone(), *w)));
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<(), EngineError> {
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| EngineError("refresh was called before create_view".into()))?;

        // Spec §8.2: merge the raw Δ by Z-set, adding up each row's weights,
        // before it reaches the operators. Merge per table — the same row value
        // appearing in two tables must not cancel across them. A `BTreeMap`
        // rather than a `HashMap` keeps the advance order deterministic
        // (§9.4), although it does not reach the output today: `ZSet::merge`
        // is pointwise addition, independent of call order. It becomes
        // observable once join lands and `ΔR⋈ΔS` reads both sides' current
        // state — item 6 of the join-landing checklist in docs/mutation-gates.md.
        let mut by_table: BTreeMap<String, ZSet> = BTreeMap::new();
        for (table, row, w) in std::mem::take(&mut self.pending) {
            by_table.entry(table).or_default().update(row, w);
        }

        tree.reset_counts();
        for (table, delta) in &by_table {
            // Rows whose net weight is 0 after merging were already removed by
            // `ZSet::update` (§5.1), so they never appear here; a table whose
            // delta is empty does not push the operator tree at all, which is
            // itself part of what `tree_pushes` should reflect.
            if delta.is_empty() {
                continue;
            }
            self.view.merge(&tree.delta(table, delta));
        }
        self.rows_processed = tree.rows_fed;
        self.tree_pushes = tree.pushes;
        Ok(())
    }

    /// The rows the last `refresh` actually pushed through the operator tree.
    ///
    /// With merging in effect, a row appearing 5 times in one batch is pushed
    /// once, and a row whose `+1` and `-1` cancel is pushed zero times. Those
    /// two numbers are consolidation's only observable footprint. It reads
    /// `CountingTree::rows_fed` — the total rows `Node::delta` really received.
    pub fn rows_processed_last_refresh(&self) -> usize {
        self.rows_processed
    }

    /// How many times the operator tree's top-level entry (`Node::delta`) was
    /// called in the last `refresh`.
    ///
    /// With consolidation in effect, each table should trigger exactly one
    /// call per `refresh`, however many raw Δ it had before merging and however
    /// many rows remain after — the direct evidence, in the call-count
    /// dimension, of the "push fewer times" performance story. `rows_processed`
    /// proves it only in the row-count dimension; together they rule out
    /// refactors like pushing row by row (final review Finding B).
    pub fn tree_pushes_last_refresh(&self) -> usize {
        self.tree_pushes
    }

    /// The view's current materialized result.
    ///
    /// Named `snapshot` rather than `materialize`: an inherent method always
    /// wins method resolution over a trait method of the same name (such as
    /// `ivmlite_test::Engine::materialize`), so with the same name any call site
    /// holding a concrete `IncrementalEngine` (rather than an `impl Engine`
    /// generic) — as `ivmlite-sqlite` will in M1b — would silently call this
    /// one instead of the trait's, with no compile-time signal. M1a Phase 2
    /// Task 5 re-review Finding 1: the shadowing had already happened once,
    /// forcing `harness_catches_bugs.rs` to work around it with UFCS
    /// (`Engine::materialize(&mut inc)`). The root cause — the name clash — is
    /// designed away here instead of worked around at every call site.
    pub fn snapshot(&self) -> ZSet {
        self.view.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, Value};

    fn table(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![Column {
                name: "a".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        }
    }

    fn count_query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        }
    }

    /// Re-review Finding 2 (M1a Phase 2 Task 5): when `create_view` declares a
    /// table but `initial` gives no state for it, it must fail rather than
    /// quietly treat the table as empty. Harness-generated cases never reach
    /// this path — `gen_initial` always fills every table in `db.tables()` —
    /// but M1b call sites that build `initial` by hand (say, the SQLite side
    /// rebuilding a view for only some tables) will hit it, so it is pinned
    /// directly in `engine.rs`'s own unit tests, independent of the
    /// differential harness's case distribution.
    #[test]
    fn create_view_errors_when_a_declared_table_has_no_initial_state() {
        let db = Database::new(vec![table("t0"), table("t1")]);
        let initial = BTreeMap::from([("t0".to_string(), ZSet::new())]); // t1 is missing
        let mut engine = IncrementalEngine::new();
        let err = engine
            .create_view(&db, &count_query(), &initial)
            .expect_err(
                "t1 has no initial state; this must fail, not be treated as an empty table",
            );
        assert!(
            err.0.contains("t1"),
            "the error must name the table missing its initial state: {}",
            err.0
        );
    }

    #[test]
    fn apply_before_create_view_is_an_error() {
        let mut engine = IncrementalEngine::new();
        let row = Row::new(vec![Value::Int(1)]);
        let err = engine
            .apply("t0", &[(row, 1)])
            .expect_err("calling apply before create_view must fail");
        assert!(err.0.contains("apply"));
    }

    #[test]
    fn refresh_before_create_view_is_an_error() {
        let mut engine = IncrementalEngine::new();
        let err = engine
            .refresh()
            .expect_err("calling refresh before create_view must fail");
        assert!(err.0.contains("refresh"));
    }

    // --- Task 6: delta consolidation ---

    fn db() -> Database {
        Database::new(vec![Schema {
            table: "t".into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        }])
    }

    fn query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        }
    }

    fn engine() -> IncrementalEngine {
        let mut e = IncrementalEngine::new();
        e.create_view(
            &db(),
            &query(),
            &BTreeMap::from([("t".to_string(), ZSet::new())]),
        )
        .unwrap();
        e
    }

    fn row(k: &str, v: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(v)])
    }

    #[test]
    fn duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators() {
        // Spec §8.2/§8.5: a row appearing 5 times in one batch is pushed once
        // after merging. This is consolidation's only observable footprint — it
        // changes the amount of work, not the result.
        let mut e = engine();
        let raw: Vec<(Row, i64)> = (0..5).map(|_| (row("a", 1), 1)).collect();
        e.apply("t", &raw).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "5 identical raw Δ must be merged into 1 before reaching the operators"
        );
        assert_eq!(
            e.tree_pushes_last_refresh(),
            1,
            "one row remains after merging, so the operator tree must be pushed exactly once"
        );
        assert_eq!(
            e.snapshot()
                .weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(5)])),
            1,
            "merging must not change the result: COUNT is still 5"
        );
    }

    #[test]
    fn rows_that_cancel_within_a_batch_never_reach_the_operators() {
        // Inserting and deleting the same row in one batch nets a weight of 0 after merging, so it should never reach the operators.
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("a", 1), -1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            0,
            "rows that cancel must not reach the operators"
        );
        assert!(e.snapshot().is_empty());
    }

    #[test]
    fn distinct_rows_are_not_over_merged() {
        // The reverse guard: merging must not fold distinct rows into one.
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("b", 1), 1), (row("a", 2), 1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            3,
            "the three rows are all distinct, so none may be merged away"
        );
        // Final review Finding B: these three rows belong to one table and one
        // refresh, and the point of consolidation is to push them through the
        // operator tree as **one batch** after merging — not once per row.
        // `rows_processed_last_refresh` can prove only "was the number of rows
        // that reached the operators right", not "was the number of calls
        // right"; the latter can be pinned only by a value counted at the
        // `Node::delta` call itself (see `CountingTree`).
        assert_eq!(
            e.tree_pushes_last_refresh(),
            1,
            "three rows of the same table in the same refresh must push the operator tree once, not once per row"
        );
    }

    #[test]
    fn deltas_for_different_tables_are_consolidated_separately() {
        // The same row value in two tables must not be merged across them —
        // that would let one table's change cancel another's. With one table
        // the shape does not exist; once join lands it is the norm.
        let two = Database::new(vec![
            Schema {
                table: "t".into(),
                columns: db().tables()[0].columns.clone(),
            },
            Schema {
                table: "u".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let mut e = IncrementalEngine::new();
        e.create_view(
            &two,
            &query(),
            &BTreeMap::from([
                ("t".to_string(), ZSet::new()),
                ("u".to_string(), ZSet::new()),
            ]),
        )
        .unwrap();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.apply("u", &[(row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            2,
            "one row in each of two tables must not cancel across them"
        );
        assert_eq!(
            e.tree_pushes_last_refresh(),
            2,
            "each of the two tables needs its own push — Scan routes by table name, and one call carries one table name"
        );
    }

    #[test]
    fn the_counter_resets_between_refreshes() {
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "the count is for the last refresh, not cumulative"
        );
    }

    // --- Final review Finding A: a failed create_view must not corrupt existing state ---

    #[test]
    fn a_failed_create_view_does_not_corrupt_existing_state() {
        // The first create_view succeeds and a batch is pushed through, establishing a correct baseline.
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        let before = e.snapshot();
        assert_eq!(
            before.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1,
            "the baseline itself must be correct: COUNT(a)=1"
        );

        // The second create_view declares tables t0 and t1 but gives initial
        // state only for t0, so the bootstrap loop must fail at t1. The previous
        // implementation had already reset self.view to an empty ZSet before
        // failing, while self.tree still held the first create_view's tree —
        // permanently inconsistent from then on (Finding A's probe: in a real
        // sequence the truth was {(a,2),(b,1)} and the engine reported
        // {(a,1),(b,1)}, never recovering).
        let two = Database::new(vec![table("t0"), table("t1")]);
        let err = e
            .create_view(
                &two,
                &count_query(),
                &BTreeMap::from([("t0".to_string(), ZSet::new())]), // t1 is missing
            )
            .expect_err("t1 has no initial state, so the second create_view must fail");
        assert!(err.0.contains("t1"));

        // The failed second create_view must not touch the existing state — the
        // snapshot must equal the pre-failure one exactly, not "roughly".
        assert_eq!(
            e.snapshot(),
            before,
            "a failed second create_view must not corrupt the existing view state"
        );

        // And the engine must still be in the usable state the first
        // create_view built: a later apply + refresh should keep advancing
        // correctly from the old view, not compute garbage on a dangling tree or
        // panic.
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        let after = e.snapshot();
        assert_eq!(
            after.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1,
            "a's count in the old state must not be damaged by the failed create_view or anything after it"
        );
        assert_eq!(
            after.weight_of(&Row::new(vec![Value::Text("b".into()), Value::Int(1)])),
            1,
            "after the failure the engine must still advance correctly on the old view"
        );
    }

    // --- Final review Finding L: apply must reject undeclared tables ---

    #[test]
    fn apply_rejects_an_unknown_table() {
        let mut e = engine(); // declares only table "t"
        let err = e
            .apply("nope", &[(row("a", 1), 1)])
            .expect_err("apply receiving a table name create_view never declared must fail");
        assert!(
            err.0.contains("nope"),
            "the error should name the undeclared table: {}",
            err.0
        );
    }
}
