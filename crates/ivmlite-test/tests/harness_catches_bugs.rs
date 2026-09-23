use ivmlite_core::{Database, IncrementalEngine, Row, ZSet};
use ivmlite_test::{
    check_batch_invariance, enumerate, gen_case, gen_case_with_query, gen_database, is_legal,
    load_regressions, recompute_via_sqlite, run, save_regression, seed_range, shrink, Agg, AggFn,
    Batching, Column, ColumnType, Domain, Engine, NaiveRecompute, NoRetractionEngine, Predicate,
    Schema, TransientDriftEngine, ViewQuery,
};
use std::collections::BTreeMap;

/// `amount` is deliberately nullable: otherwise the "SUM over zero non-NULL
/// inputs" path is never reached by random testing, and spec §6.1's
/// NULL-semantics contract has unit-test coverage only, no differential coverage.
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
                nullable: true,
            },
        ],
    }
}

fn db() -> Database {
    Database::single(schema())
}

#[test]
fn naive_engine_is_green_across_many_seeds() {
    let db = db();
    let domain = Domain::default();
    for seed in seed_range() {
        let case = gen_case(seed, &db, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case)
            .unwrap_or_else(|f| panic!("the reference implementation should not fail: {f}"));
    }
}

#[test]
fn naive_engine_satisfies_batch_invariance() {
    let db = db();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(10) {
        let case = gen_case(seed, &db, &domain, 25, 120, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap_or_else(|f| {
            panic!("the reference implementation should not violate batch independence: {f}")
        });
    }
}

/// Final review Finding K: `check_batch_invariance` covers four modes
/// internally — `Batching::All` / `One` / `Chunks(3)` / `Chunks(17)` — but until
/// this test was added, `IncrementalEngine` had never been run that way. Both
/// existing integration tests
/// (`incremental_engine_is_green_across_the_enumerated_space` and
/// `incremental_engine_matches_naive_recompute_at_every_refresh_point`) use only
/// `Batching::Chunks`, so the real incremental engine had never been
/// differentially compared under `All` (ingesting the whole batch at once) or
/// `One` (ingesting one delta at a time).
#[test]
fn incremental_engine_satisfies_batch_invariance() {
    let db = gen_database(2);
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(10) {
        let case = gen_case(seed, &db, &domain, 25, 120, Batching::All);
        check_batch_invariance(&case, IncrementalEngine::new).unwrap_or_else(|f| {
            panic!("the incremental engine should not violate batch independence: {f}")
        });
    }
}

/// Frozen historical failing cases must always pass. In M0 the reference
/// implementation was trivially correct, so this test only built the mechanism;
/// M1a Phase 2, which plugged in the real incremental engine, is when the
/// replay started to catch anything —
/// `saved_regressions_still_pass_against_incremental_engine` below makes that a
/// real regression replay rather than an unfulfilled promise (final review
/// Finding J). This test itself still runs only `NaiveRecompute`: it proves
/// "the reference implementation is still trivially correct on the regression
/// cases", a different thing from what the test below proves.
#[test]
fn saved_regressions_still_pass() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    for case in load_regressions(&dir).expect("failed to read the regressions directory") {
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("a regression case failed: {f}"));
    }
}

/// Final review Finding J: M1a Phase 2 is the "M1 plugs in the real engine" the
/// comment above describes, yet until this test was added the regression replay
/// had never actually run `IncrementalEngine` — only `NaiveRecompute` (the
/// trivially correct reference) and `NoRetractionEngine` (the deliberately buggy
/// counterexample). This replays the frozen regression cases on the real engine
/// too, showing they are not only well-formed but still correct on the engine
/// that will actually maintain views.
#[test]
fn saved_regressions_still_pass_against_incremental_engine() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    for case in load_regressions(&dir).expect("failed to read the regressions directory") {
        let mut engine = IncrementalEngine::new();
        run(&mut engine, &case)
            .unwrap_or_else(|f| panic!("a regression case failed on IncrementalEngine: {f}"));
    }
}

/// Shows that comparing against the oracle per batch has independent value: it
/// catches an implementation that "goes wrong midway, stays well-formed, and
/// later heals itself".
///
/// This is the only evidence for the O(n × base-table size) cost spec §9.1 pays
/// for per-batch comparison. The assertion has two halves: run must fail at
/// **some intermediate point other than bootstrap**, while the same engine
/// replayed to the end by hand reaches a final state that **agrees** with the
/// oracle — "correct at the end + run fails" is exactly why comparing only the
/// final state would miss it.
///
/// A note on the m5 rename: this test used to be called
/// `per_batch_oracle_comparison_catches_transient_drift`, but its assertion was
/// too loose to keep the name's promise of "per-batch comparison" — deleting the
/// per-batch comparison inside `differential::run`'s loop and comparing once
/// after the loop still let it pass (details in `docs/mutation-gates.md`). The
/// gap is closed by `oracle_comparison_runs_after_every_batch_not_only_at_the_end`
/// below. What this test really pins, and always holds, is what its name now
/// says: replayed to the end by hand, the final state must be correct.
#[test]
fn transient_drift_has_a_correct_final_state() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(3, &db, &domain, 25, 150, Batching::Chunks(5));
    let table = case.database.tables()[0].table.clone();

    // drift_at = 2: the first materialize is the bootstrap, the second comes after the first batch
    let mut engine = TransientDriftEngine::new(2);
    let failure =
        run(&mut engine, &case).expect_err("per-batch comparison must catch the midway drift");
    assert!(
        failure.stage.starts_with("diff["),
        "it should fail at the oracle comparison, got stage={}",
        failure.stage
    );
    assert_ne!(
        failure.stage, "diff[bootstrap]",
        "the drift is set after the first batch and should not be reported at bootstrap"
    );

    // Replay to the end by hand, showing this engine's final state is correct
    let mut settled = TransientDriftEngine::new(2);
    let mut base = ZSet::from_rows(
        case.initial
            .get(&table)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r, 1)),
    );
    let bases = BTreeMap::from([(table.clone(), base.clone())]);
    settled
        .create_view(&case.database, &case.query, &bases)
        .unwrap();
    let _ = settled.materialize().unwrap(); // call 1: bootstrap

    // Unconsolidated raw deltas — matching the signature, the engine decides for
    // itself whether to consolidate. The harness-side `base` is merged as usual
    // to feed the oracle.
    let raw: Vec<(Row, i64)> = case.ops.iter().flat_map(|(_, op)| op.to_delta()).collect();
    for (row, w) in &raw {
        base.update(row.clone(), *w);
    }
    settled.apply(&table, &raw).unwrap();
    settled.refresh().unwrap();
    let _ = settled.materialize().unwrap(); // call 2: the corrupted one
    let settled_state = settled.materialize().unwrap(); // call 3: recovered

    let bases = BTreeMap::from([(table, base)]);
    let want = recompute_via_sqlite(&case.database, &case.query, &bases).unwrap();
    assert_eq!(
        settled_state, want,
        "the final state must be correct — which is exactly why comparing only the final state misses this bug"
    );
}

/// A gap test (added by M1a Phase 1 Task 1's mutation audit):
/// `transient_drift_has_a_correct_final_state` (named
/// `per_batch_oracle_comparison_catches_transient_drift` before the m5 rename)
/// had too loose an assertion — it only required the failing stage to match
/// `diff[...]` and not be `diff[bootstrap]`, and `TransientDriftEngine::new(2)`'s
/// second `materialize` call lands on the call right after the first whether
/// `run` "compares after every batch" or "compares once after the loop", so both
/// implementations turned that test green. Verified by mutation (deleting the
/// per-batch `compare` inside `differential::run`'s loop and comparing once after
/// the loop instead): that test did not go red, showing spec §9.1's "compare
/// against the oracle at every refresh point" was not in fact guarded.
///
/// This test breaks the coincidence with a `drift_at` in the **middle** of the
/// batch sequence (rather than the second call right after bootstrap): under
/// the correct implementation `materialize` is called once per batch, `drift_at`
/// hits some middle batch, and `run` must fail exactly at that batch's
/// `diff[<i>]`; an implementation that "compares once after the loop" calls
/// `materialize` only twice in all (bootstrap and once at the end), never
/// reaches a `drift_at` placed deliberately in the middle, reports only
/// "correct" states throughout, and `run` returns `Ok` instead of the expected
/// `Err`.
#[test]
fn oracle_comparison_runs_after_every_batch_not_only_at_the_end() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(3, &db, &domain, 25, 150, Batching::Chunks(5));

    // Batching::Chunks(5) turns 150 operations into 30 batches. Under correct
    // behaviour the materialize calls are: call 1 = bootstrap, call (k+2) = after
    // batch k (k from 0). drift_at = 16 lands on batch i = 14 — neither the
    // bootstrap nor either of the only two calls (bootstrap and the end) that a
    // "compare once at the end" implementation makes.
    let drift_at = 16;
    let expected_batch = drift_at - 2;

    let mut engine = TransientDriftEngine::new(drift_at);
    let failure = run(&mut engine, &case).expect_err(
        "per-batch comparison must catch the drift right after a middle batch; comparing \
         only once after the loop would never trigger this deliberately mid-sequence \
         drift_at, and run would falsely report success",
    );
    assert_eq!(
        failure.stage,
        format!("diff[{expected_batch}]"),
        "it must fail exactly at the comparison after batch {expected_batch} — direct evidence \
         of per-batch comparison (rather than comparing once); got stage={}",
        failure.stage
    );
}

/// I3: makes good on the comment on `run`'s bootstrap comparison — "so a case
/// with no ops is genuinely checked too". Before this test, no `gen_case` call
/// anywhere in the codebase passed `op_count == 0`, so that comment had never
/// been verified.
#[test]
fn zero_op_case_still_gets_checked() {
    let db = db();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(5) {
        let case = gen_case(seed, &db, &domain, 25, 0, Batching::Chunks(5));
        assert!(case.ops.is_empty());
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("a zero-op case should not fail: {f}"));
    }
}

/// I3's substantive guard: the oracle comparison after bootstrap (the
/// `compare(engine, &base, "bootstrap")` line in `differential.rs`) is the only
/// place `create_view`'s correctness is checked. Delete it, and an engine that
/// computes the initial state wrong at bootstrap but never errs afterwards
/// fools the whole `run` — especially when `op_count == 0`, since no later batch
/// comparison exists to catch it along the way. M1's bootstrap watermark
/// atomicity (spec §7.3) is exactly what is most likely to go wrong at this point.
#[test]
fn bootstrap_drift_is_caught_at_the_bootstrap_stage() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(1, &db, &domain, 25, 0, Batching::Chunks(5));
    assert!(
        case.ops.is_empty(),
        "the only materialize call must be the bootstrap itself"
    );

    // drift_at = 1: the first (and only) materialize call is the bootstrap.
    let mut engine = TransientDriftEngine::new(1);
    let failure =
        run(&mut engine, &case).expect_err("a wrong initial state at bootstrap must be caught");
    assert_eq!(
        failure.stage, "diff[bootstrap]",
        "without the bootstrap comparison this op_count=0 case would run through without any error"
    );
}

/// M0's first acceptance criterion: the framework must catch the planted bug.
#[test]
fn harness_catches_the_missing_retraction_bug() {
    let db = db();
    let domain = Domain::default();
    let seeds = seed_range();
    let total = seeds.len();
    let mut caught = 0;
    for seed in seeds {
        let case = gen_case(seed, &db, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NoRetractionEngine::new();
        if run(&mut engine, &case).is_err() {
            caught += 1;
        }
    }
    assert!(
        caught * 10 >= total * 9,
        "only {caught} of {total} seeds caught the bug — the generator's detection rate is \
         too low, so the value domain or the biased-sampling parameters need adjusting; \
         do not relax this assertion"
    );
}

/// M0's second acceptance criterion: a failing case must shrink to 10 steps or
/// fewer and be frozen as a regression case.
#[test]
fn failing_case_shrinks_to_under_ten_ops() {
    let db = db();
    let domain = Domain::default();

    let case = seed_range()
        .into_iter()
        .map(|seed| gen_case(seed, &db, &domain, 25, 150, Batching::Chunks(5)))
        .find(|c| {
            let mut engine = NoRetractionEngine::new();
            run(&mut engine, c).is_err()
        })
        .expect("there should be at least one failing case");

    let minimal = shrink(&case, NoRetractionEngine::new);

    // The shrunk case itself must still be legal — if the shrinker's legality
    // gate (spec §9.3) were broken, the sequence could contain a dangling
    // DELETE/UPDATE, illegal input the engine was never obliged to handle.
    assert!(
        is_legal(&minimal.initial, &minimal.ops),
        "shrink's output must always be legal: {minimal:?}"
    );

    let mut engine = NoRetractionEngine::new();
    let failure = run(&mut engine, &minimal).expect_err("the shrunk case must still fail");
    // With the gate broken (is_legal always true), an illegal sequence drives
    // some group's weight negative and recompute_via_sqlite rejects it as
    // `oracle[...]` — an artifact unrelated to the original bug, which the ≤10
    // step assertion cannot tell apart. A real failure must land in the invariant
    // layer or as an oracle diff, not as the oracle rejecting its input.
    assert!(
        !failure.stage.starts_with("oracle["),
        "the shrunk case fails at stage={} — exactly the artifact a broken legality gate \
         converges to, not a failure of the same family as the original bug",
        failure.stage
    );
    assert!(
        minimal.ops.len() <= 10,
        "spec §11's M0 requires shrinking to 10 steps or fewer, got {} steps",
        minimal.ops.len()
    );

    // Capture goes to the system temp directory by default and never dirties the
    // tracked working tree (C1): the fixtures under `tests/regressions/` are
    // inputs, not `cargo test` outputs. Only with IVMLITE_CAPTURE=1 set does it
    // write back into the tracked directory, to freeze a new case by hand; from
    // then on saved_regressions_still_pass and
    // saved_regressions_still_reproduce_their_original_failure guard it.
    let dir = if std::env::var("IVMLITE_CAPTURE").as_deref() == Ok("1") {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions")
    } else {
        std::env::temp_dir().join("ivmlite-test-captured-regressions")
    };
    let path = save_regression(&dir, &minimal).expect("failed to freeze the regression case");
    eprintln!("froze the minimal case: {}", path.display());
}

/// C1's core assertion: a committed fixture must not merely deserialize, it must
/// still reproduce the failure it was captured with — replaying it against
/// NoRetractionEngine must still give `Err`.
///
/// This proves something different from `saved_regressions_still_pass`
/// (replaying against NaiveRecompute and expecting `Ok`): that one shows "the
/// reference implementation is still trivially correct on the regression
/// cases"; this one shows "each regression case is still a genuine witness of a
/// failure, not silently replaced by something else after the shrinker's
/// behaviour changed".
#[test]
fn saved_regressions_still_reproduce_their_original_failure() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    let cases = load_regressions(&dir).expect("failed to read the regressions directory");
    assert!(
        !cases.is_empty(),
        "the regressions directory should not be empty — it should hold at least seed0-ops1.json"
    );
    for case in cases {
        let mut engine = NoRetractionEngine::new();
        assert!(
            run(&mut engine, &case).is_err(),
            "regression case seed={} must still make NoRetractionEngine fail, or this fixture has gone stale",
            case.seed
        );
    }
}

/// This phase's deliverable: the framework can express multi-table cases. The
/// query is still a single-table aggregate (join is in the engine plan's
/// Phase 3), but both tables receive changes, so three code paths are genuinely
/// executed: apply's table-name routing, `NaiveRecompute`'s per-table base /
/// pending storage, and the oracle creating and loading every table in the
/// `Database`.
///
/// "Executed" is not "this test would catch it breaking" — only the first of
/// the three is caught:
/// - apply's table-name routing: **caught**. Hard-coding the table name `run`
///   passes to `apply` as `db.tables()[0].table` reddens this test at `diff[0]`
///   (measured by the review; see the M1a Phase 1 Task 5 row in
///   `docs/mutation-gates.md`).
/// - `NaiveRecompute`'s per-table base / pending: **not caught**. Phase 1's query
///   and oracle render only the anchor table's (`db.tables()[0]`) single-table
///   SQL, so the non-anchor state stored there is unobservable in both
///   `materialize()` and the oracle comparison — silently dropping it does not
///   redden this test. It is a registered known gap: see the row "§8.5 `apply`
///   must really keep non-anchor tables' deltas" in `docs/mutation-gates.md`,
///   to be re-verified once join lands and the oracle renders multi-table queries.
/// - the oracle creating and loading every table: **not caught** here (it is
///   guarded separately by `oracle::tests::builds_every_table_in_the_database`).
#[test]
fn a_two_table_case_runs_green_against_the_reference_engine() {
    let db = gen_database(2);
    for seed in seed_range() {
        let case = gen_case(seed, &db, &Domain::default(), 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case)
            .unwrap_or_else(|f| panic!("the reference implementation should not fail: {f}"));
    }
}

/// I4: `shrink`'s phase 3 (deleting initial data row by row, table by table) had
/// never run on a genuinely multi-table case — the review's probe
/// `assert!(case.database.len() <= 1)` confirmed it. This finds a two-table case
/// that makes `NoRetractionEngine` fail, feeds it to `shrink`, and asserts that
/// both tables' initial row counts were really reduced, not just the one table
/// the loop happens to meet first.
///
/// The query in this case still reads only the anchor table (`t0`) — query
/// rendering always uses the anchor, an existing Phase 1 limitation — so `t1` is
/// completely unobservable to the oracle comparison, and a correct shrink should
/// reduce it to 0 rows. That is the signal this test uses to tell "phase 3
/// processed every table" from "phase 3 processed only the first table": if the
/// loop handled only the first table (or never reached `t1`), `t1` would keep
/// all 25 of its initial rows.
#[test]
fn shrink_reduces_initial_rows_in_every_table_of_a_multi_table_case() {
    let db = gen_database(2);
    let domain = Domain::default();

    let case = seed_range()
        .into_iter()
        .map(|seed| gen_case(seed, &db, &domain, 25, 150, Batching::Chunks(5)))
        .find(|c| {
            let mut engine = NoRetractionEngine::new();
            run(&mut engine, c).is_err()
        })
        .expect("there should be at least one failing two-table case");

    // I4's probe: phase 3's per-table loop must really run on a case with more
    // than one table in `initial`, not merely "accept several tables in its
    // signature and never be called that way".
    assert_eq!(
        case.database.len(),
        2,
        "this test must feed shrink a genuine two-table case"
    );
    assert!(
        case.initial.keys().count() > 1,
        "the best.initial.keys() that phase 3 iterates over must start with more than one table"
    );

    let minimal = shrink(&case, NoRetractionEngine::new);

    // The shrinker's legality gate (spec §9.3) must still hold on the multi-table path.
    assert!(
        is_legal(&minimal.initial, &minimal.ops),
        "shrink's output must always be legal: {minimal:?}"
    );

    let mut engine = NoRetractionEngine::new();
    let failure = run(&mut engine, &minimal).expect_err("the shrunk case must still fail");
    // The same reasoning as failing_case_shrinks_to_under_ten_ops: if is_legal were
    // broken, this assertion is the only thing that tells "an artifact" apart from
    // "a failure of the same family as the original bug" — and on the
    // multi-table path the gate is especially easy to get wrong (legitimising a
    // delete on table B with a row from table A), so it must really hold here too.
    assert!(
        !failure.stage.starts_with("oracle["),
        "the shrunk case fails at stage={} — the artifact a broken legality gate converges to",
        failure.stage
    );

    let rows_by_table: BTreeMap<&str, usize> = minimal
        .initial
        .iter()
        .map(|(t, rows)| (t.as_str(), rows.len()))
        .collect();
    assert!(
        rows_by_table.values().all(|&n| n < 25),
        "phase 3 must really have deleted rows from every table; no table may keep all 25 of \
         its initial rows: {rows_by_table:?}"
    );
    let total_rows: usize = rows_by_table.values().sum();
    assert!(
        total_rows <= 10,
        "the two tables' combined initial rows should converge to single digits — the query \
         reads only the anchor table t0, the non-anchor t1 is completely unobservable to the \
         oracle comparison, and a correct shrink should reduce it to 0 rows; got \
         {rows_by_table:?}"
    );
}

/// The M1a checkpoint (spec §11): the differential framework running green on a
/// real incremental engine for the first time.
///
/// This test has the same structure as `naive_engine_is_green_across_many_seeds`,
/// with `IncrementalEngine` as the subject — it is incremental while the
/// reference recomputes in full, and both must agree with the oracle at every
/// refresh point.
#[test]
fn incremental_engine_is_green_across_the_enumerated_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate(&db.tables()[0]);
    // Final review Finding D: this test's name promises "covers the whole
    // enumerated query space", but the old assertion (`checked >= 50`) counted
    // **seeds run**, not **queries covered** — the two coincided only because
    // `seed_range()` yields 50 seeds by default and `enumerate` currently
    // produces fewer queries than that. Counting the `exercised` index set makes
    // the assertion verify what the name says: even if `enumerate` someday
    // produces more than 50 queries, coverage is not silently lost here.
    let mut exercised: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut checked = 0usize;
    for seed in seed_range() {
        let idx = seed as usize % queries.len();
        let query = &queries[idx];
        let case = gen_case_with_query(
            seed,
            &db,
            &domain,
            query.clone(),
            20,
            60,
            Batching::Chunks(4),
        );
        let mut engine = IncrementalEngine::new();
        if let Err(f) = run(&mut engine, &case) {
            panic!("the incremental engine disagrees with the oracle at seed={seed}: {f}");
        }
        exercised.insert(idx);
        checked += 1;
    }
    assert!(checked >= 1, "at least one seed must run, ran {checked}");

    // In `IVMLITE_SEED` single-seed replay mode only one query runs, so "cover the
    // whole enumerated space" does not apply — the replay command
    // `Failure::Display` prints is precisely `IVMLITE_SEED=<seed> cargo test ...`,
    // and if this assertion still demanded every enumerated query in single-seed
    // mode, following that replay instruction would first hit a red unrelated to
    // the original bug (final review Finding D).
    if std::env::var("IVMLITE_SEED").is_err() {
        assert_eq!(
            exercised.len(),
            queries.len(),
            "every one of the {} queries enumerate produces must be covered; only {} were: {:?}",
            queries.len(),
            exercised.len(),
            exercised
        );
    }
}

/// The incremental engine must agree with full recomputation at **every refresh
/// point**, not only at the final state. TransientDriftEngine exists to prove
/// the two are not the same thing (spec §9.1).
#[test]
fn incremental_engine_matches_naive_recompute_at_every_refresh_point() {
    let db = gen_database(2);
    let case = gen_case(11, &db, &Domain::default(), 25, 120, Batching::Chunks(5));

    let mut inc = IncrementalEngine::new();
    let mut naive = NaiveRecompute::new();
    let bases: BTreeMap<String, ZSet> = case
        .initial
        .iter()
        .map(|(t, rows)| {
            (
                t.clone(),
                ZSet::from_rows(rows.iter().map(|r| (r.clone(), 1))),
            )
        })
        .collect();
    inc.create_view(&case.database, &case.query, &bases)
        .unwrap();
    naive
        .create_view(&case.database, &case.query, &bases)
        .unwrap();
    assert_eq!(
        inc.materialize().unwrap(),
        naive.materialize().unwrap(),
        "they already disagree at bootstrap"
    );

    for batch in case.batches() {
        for (table, raw) in &batch {
            inc.apply(table, raw).unwrap();
            naive.apply(table, raw).unwrap();
        }
        inc.refresh().unwrap();
        naive.refresh().unwrap();
        assert_eq!(
            inc.materialize().unwrap(),
            naive.materialize().unwrap(),
            "incremental maintenance and full recomputation diverge at a refresh point"
        );
    }
}

/// Spec §5.2's boundary check must take effect at create_view, not as a panic at refresh.
#[test]
fn create_view_rejects_a_global_aggregate() {
    let db = gen_database(1);
    let bad = ViewQuery {
        group_by: vec![],
        aggs: vec![Agg {
            func: AggFn::Count,
            column: None,
        }],
        predicate: Predicate::None,
        join: None,
    };
    let mut engine = IncrementalEngine::new();
    let err = engine
        .create_view(
            &db,
            &bad,
            &BTreeMap::from([(db.tables()[0].table.clone(), ZSet::new())]),
        )
        .expect_err("an empty group_by must be rejected at create_view");
    assert!(
        err.0.contains("GROUP BY") || err.0.contains("group_by"),
        "the error should name group_by: {}",
        err.0
    );
}
