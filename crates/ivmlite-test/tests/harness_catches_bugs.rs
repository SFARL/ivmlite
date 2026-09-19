use ivmlite_core::ZSet;
use ivmlite_test::{
    check_batch_invariance, gen_case, load_regressions, recompute_via_sqlite, run, save_regression,
    seed_range, shrink, Batching, Column, ColumnType, Domain, Engine, NaiveRecompute,
    NoRetractionEngine, Schema, TransientDriftEngine,
};

/// `amount` 刻意可空：否则"SUM 的非 NULL 输入为零行"这条路径在随机测试里
/// 永远走不到，spec §6.1 的 NULL 语义契约就只有单元测试覆盖，没有差分覆盖。
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

#[test]
fn naive_engine_is_green_across_many_seeds() {
    let schema = schema();
    let domain = Domain::default();
    for seed in seed_range() {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("参照实现不应失败: {f}"));
    }
}

#[test]
fn naive_engine_satisfies_batch_invariance() {
    let schema = schema();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(10) {
        let case = gen_case(seed, &schema, &domain, 25, 120, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new)
            .unwrap_or_else(|f| panic!("参照实现不应违反批次无关性: {f}"));
    }
}

/// 固化下来的历史失败用例必须始终通过。M0 里参照实现平凡正确，因此这个测试
/// 的作用是把机制建起来；它真正开始拦 bug 是在 M1 接入真实引擎之后。
#[test]
fn saved_regressions_still_pass() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    for case in load_regressions(&dir).expect("读取回归用例目录失败") {
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("回归用例失败: {f}"));
    }
}

/// 证明逐批比对 oracle 有独立价值：抓到「中途算错、形式合法、之后自愈」的实现。
///
/// 这是 spec §9.1 为逐批比对付出 O(n × 基表规模) 代价的唯一证据。
/// 断言分两半：run 必须在**非 bootstrap 的某个中间点**失败；而同一个引擎
/// 手工重放到底之后，最终状态与 oracle **一致**——「末尾正确 + run 失败」
/// 正说明只比最终状态会漏掉它。
#[test]
fn per_batch_oracle_comparison_catches_transient_drift() {
    let schema = schema();
    let domain = Domain::default();
    let case = gen_case(3, &schema, &domain, 25, 150, Batching::Chunks(5));

    // drift_at = 2：第 1 次 materialize 是 bootstrap，第 2 次是第一批之后
    let mut engine = TransientDriftEngine::new(2);
    let failure = run(&mut engine, &case).expect_err("逐批比对必须抓到中途漂移");
    assert!(
        failure.stage.starts_with("diff["),
        "应当在 oracle 比对处失败，实得 stage={}",
        failure.stage
    );
    assert_ne!(
        failure.stage, "diff[bootstrap]",
        "漂移设定在第一批之后，不应在 bootstrap 处报出"
    );

    // 手工重放到底：证明这个引擎的最终状态是正确的
    let mut settled = TransientDriftEngine::new(2);
    let mut base = ZSet::from_rows(case.initial.iter().cloned().map(|r| (r, 1)));
    settled
        .create_view(&case.schema, &case.query, &base)
        .unwrap();
    let _ = settled.materialize().unwrap(); // call 1: bootstrap

    let mut all = ZSet::new();
    for op in &case.ops {
        for (row, w) in op.to_delta() {
            all.update(row.clone(), w);
            base.update(row, w);
        }
    }
    settled.apply(&all).unwrap();
    let _ = settled.materialize().unwrap(); // call 2: 被污染的那次
    let settled_state = settled.materialize().unwrap(); // call 3: 已恢复

    let want = recompute_via_sqlite(&case.schema, &case.query, &base).unwrap();
    assert_eq!(
        settled_state, want,
        "末尾状态必须正确——这正是只比最终状态会漏掉这个 bug 的原因"
    );
}

/// M0 完成判定其一：框架必须抓到植入的 bug。
#[test]
fn harness_catches_the_missing_retraction_bug() {
    let schema = schema();
    let domain = Domain::default();
    let seeds = seed_range();
    let total = seeds.len();
    let mut caught = 0;
    for seed in seeds {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NoRetractionEngine::new();
        if run(&mut engine, &case).is_err() {
            caught += 1;
        }
    }
    assert!(
        caught * 10 >= total * 9,
        "{total} 个 seed 中只抓到 {caught} 个——生成器的 bug 检出率过低，\
         说明值域或有偏采样的参数需要调整；不要放宽本断言"
    );
}

/// M0 完成判定其二：失败用例必须能缩到 10 步以内，并被固化成回归用例。
#[test]
fn failing_case_shrinks_to_under_ten_ops() {
    let schema = schema();
    let domain = Domain::default();

    let case = seed_range()
        .into_iter()
        .map(|seed| gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5)))
        .find(|c| {
            let mut engine = NoRetractionEngine::new();
            run(&mut engine, c).is_err()
        })
        .expect("应当至少有一个失败用例");

    let minimal = shrink(&case, NoRetractionEngine::new);

    let mut engine = NoRetractionEngine::new();
    assert!(run(&mut engine, &minimal).is_err(), "缩小后必须仍然失败");
    assert!(
        minimal.ops.len() <= 10,
        "spec §11 M0 要求缩到 10 步以内，实得 {} 步",
        minimal.ops.len()
    );

    // 固化：写进 tests/regressions/，此后由 saved_regressions_still_pass 守着。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    let path = save_regression(&dir, &minimal).expect("固化回归用例失败");
    eprintln!("已固化最小用例: {}", path.display());
}
