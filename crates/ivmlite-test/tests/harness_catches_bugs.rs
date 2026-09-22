use ivmlite_core::{Database, IncrementalEngine, Row, ZSet};
use ivmlite_test::{
    check_batch_invariance, enumerate, gen_case, gen_case_with_query, gen_database, is_legal,
    load_regressions, recompute_via_sqlite, run, save_regression, seed_range, shrink, Agg, AggFn,
    Batching, Column, ColumnType, Domain, Engine, NaiveRecompute, NoRetractionEngine, Predicate,
    Schema, TransientDriftEngine, ViewQuery,
};
use std::collections::BTreeMap;

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
        run(&mut engine, &case).unwrap_or_else(|f| panic!("参照实现不应失败: {f}"));
    }
}

#[test]
fn naive_engine_satisfies_batch_invariance() {
    let db = db();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(10) {
        let case = gen_case(seed, &db, &domain, 25, 120, Batching::All);
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
///
/// m5 更名说明：这个名字曾叫 `per_batch_oracle_comparison_catches_transient_drift`，
/// 但它实际钉死的断言太松，守不住名字里"逐批比对"这个承诺——把
/// `differential::run` 里循环内的逐批比对整个删掉、改成循环结束后只比对
/// 一次，这个测试依然通过（细节见 `docs/mutation-gates.md`）。真正堵住那个
/// 缺口的是下面新增的 `oracle_comparison_runs_after_every_batch_not_only_at_the_end`。
/// 这个测试真正钉死、且始终成立的是它名字现在说的这件事：手工重放到底之后
/// 最终状态必须正确。
#[test]
fn transient_drift_has_a_correct_final_state() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(3, &db, &domain, 25, 150, Batching::Chunks(5));
    let table = case.database.tables()[0].table.clone();

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

    // 未合并的原始 raw delta——与新签名一致，engine 自己决定要不要 consolidate。
    // harness 侧的 `base` 仍然照常合并，用来喂 oracle。
    let raw: Vec<(Row, i64)> = case.ops.iter().flat_map(|(_, op)| op.to_delta()).collect();
    for (row, w) in &raw {
        base.update(row.clone(), *w);
    }
    settled.apply(&table, &raw).unwrap();
    settled.refresh().unwrap();
    let _ = settled.materialize().unwrap(); // call 2: 被污染的那次
    let settled_state = settled.materialize().unwrap(); // call 3: 已恢复

    let bases = BTreeMap::from([(table, base)]);
    let want = recompute_via_sqlite(&case.database, &case.query, &bases).unwrap();
    assert_eq!(
        settled_state, want,
        "末尾状态必须正确——这正是只比最终状态会漏掉这个 bug 的原因"
    );
}

/// 缺口测试（M1a Phase 1 Task 1 变异审计新增）：`transient_drift_has_a_correct_final_state`
/// （m5 更名前叫 `per_batch_oracle_comparison_catches_transient_drift`）
/// 的断言太松——它只要求失败 stage 匹配 `diff[...]` 且不是 `diff[bootstrap]`，
/// 而 `TransientDriftEngine::new(2)` 的第 2 次 `materialize` 调用，无论 `run`
/// 是"每批都比对"还是"只在循环结束后比对一次"，都恰好落在第一次之后的下一次
/// 调用上——两种实现都会让该测试变绿。用变异验证时（把 `differential::run`
/// 里循环内的逐批 `compare` 删掉、改成循环结束后只 `compare` 一次），那条
/// 测试确实没有变红，说明 spec §9.1"每个 refresh 点都比对 oracle"这条要求
/// 事实上没有被守住。
///
/// 这里用一个落在批次序列**中段**的 `drift_at`（而非紧跟 bootstrap 之后的第
/// 2 次调用）来打破这个巧合：正确实现下，`materialize` 每批调用一次，
/// `drift_at` 会命中某个中间批次，`run` 必须恰好在那个批次的 `diff[<i>]`
/// 处失败；而"只在循环结束后比对一次"的实现全程只调用两次 `materialize`
/// （bootstrap + 结束时一次），永远追不上一个刻意设在中段的 `drift_at`，
/// 于是引擎全程只会汇报"正确"的状态，`run` 会返回 `Ok`，而不是期望的 `Err`。
#[test]
fn oracle_comparison_runs_after_every_batch_not_only_at_the_end() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(3, &db, &domain, 25, 150, Batching::Chunks(5));

    // Batching::Chunks(5) 对 150 步操作产出 30 批。正确行为下 materialize
    // 的调用序列是：call 1 = bootstrap，call (k+2) = 第 k 批（k 从 0 开始）
    // 之后。drift_at = 16 落在批次 i = 14——既不是 bootstrap，也不是"只在
    // 结束时比对一次"实现下唯二会发生的两次调用（bootstrap 与结束）之一。
    let drift_at = 16;
    let expected_batch = drift_at - 2;

    let mut engine = TransientDriftEngine::new(drift_at);
    let failure = run(&mut engine, &case).expect_err(
        "逐批比对必须在中段某一批之后就抓到漂移；若只在循环结束后比对一次，\
         这个刻意设在中段的 drift_at 永远不会被触发，run 会误报成功",
    );
    assert_eq!(
        failure.stage,
        format!("diff[{expected_batch}]"),
        "必须恰好在第 {expected_batch} 批之后的比对处失败——这是逐批比对（而非只比对一次）的直接证据，实得 stage={}",
        failure.stage
    );
}

/// I3：兑现 `run` 里 bootstrap 比对那一行自己的注释——"空 ops 的用例也因此
/// 被真正检查到"。在这条测试之前，代码库里没有任何一处 `gen_case` 传入
/// `op_count == 0`，所以这句注释从未被验证过。
#[test]
fn zero_op_case_still_gets_checked() {
    let db = db();
    let domain = Domain::default();
    for seed in seed_range().into_iter().take(5) {
        let case = gen_case(seed, &db, &domain, 25, 0, Batching::Chunks(5));
        assert!(case.ops.is_empty());
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("零 ops 用例不应失败: {f}"));
    }
}

/// I3 的实质守卫：bootstrap 之后的 oracle 比对（`differential.rs` 的
/// `compare(engine, &base, "bootstrap")` 那一行）是唯一检查 `create_view`
/// 正确性的地方。删掉它，一个在 bootstrap 时就算错初始状态、但此后不再
/// 出错的引擎会骗过整个 `run`——尤其是在 `op_count == 0` 时，因为根本没有
/// 后续批次的比对能顺带抓到它。M1 的 bootstrap 水位原子性（spec §7.3）
/// 正是最容易在这个点出错的地方。
#[test]
fn bootstrap_drift_is_caught_at_the_bootstrap_stage() {
    let db = db();
    let domain = Domain::default();
    let case = gen_case(1, &db, &domain, 25, 0, Batching::Chunks(5));
    assert!(
        case.ops.is_empty(),
        "唯一一次 materialize 调用必须是 bootstrap 本身"
    );

    // drift_at = 1：第 1 次（也是唯一一次）materialize 调用就是 bootstrap。
    let mut engine = TransientDriftEngine::new(1);
    let failure = run(&mut engine, &case).expect_err("bootstrap 的错误初始状态必须被抓到");
    assert_eq!(
        failure.stage, "diff[bootstrap]",
        "bootstrap 比对若被删掉，这个 op_count=0 的用例会完全跑通而不报任何错误"
    );
}

/// M0 完成判定其一：框架必须抓到植入的 bug。
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
        "{total} 个 seed 中只抓到 {caught} 个——生成器的 bug 检出率过低，\
         说明值域或有偏采样的参数需要调整；不要放宽本断言"
    );
}

/// M0 完成判定其二：失败用例必须能缩到 10 步以内，并被固化成回归用例。
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
        .expect("应当至少有一个失败用例");

    let minimal = shrink(&case, NoRetractionEngine::new);

    // 缩小后的用例本身必须仍然合法——shrinker 的合法性门禁（spec §9.3）若
    // 被打穿，产出的序列可能包含悬空 DELETE/UPDATE，是引擎本就不该处理的
    // 非法输入。
    assert!(
        is_legal(&minimal.initial, &minimal.ops),
        "shrink 的产出必须始终合法：{minimal:?}"
    );

    let mut engine = NoRetractionEngine::new();
    let failure = run(&mut engine, &minimal).expect_err("缩小后必须仍然失败");
    // 门禁损坏时（is_legal 恒真），非法序列会让某个 group 的权重变负，
    // recompute_via_sqlite 会以 `oracle[...]` 拒绝——这是一个与原始 bug 无关
    // 的伪产物，而 ≤10 步的断言察觉不到这个区别。真实的失败必须落在
    // 不变量层或 oracle 差异层，而不是 oracle 自己拒绝了输入。
    assert!(
        !failure.stage.starts_with("oracle["),
        "缩小后的用例在 stage={} 失败——这正是合法性门禁损坏时会收敛到的伪产物形态，\
         而不是原始 bug 的同族失败",
        failure.stage
    );
    assert!(
        minimal.ops.len() <= 10,
        "spec §11 M0 要求缩到 10 步以内，实得 {} 步",
        minimal.ops.len()
    );

    // 捕获路径默认写到系统临时目录，绝不弄脏被跟踪的工作区（C1）：
    // `tests/regressions/` 下的 fixture 是输入,不是 `cargo test` 的输出。
    // 只有显式设置 IVMLITE_CAPTURE=1 时才写回被跟踪目录，用来手动固化新用例;
    // 此后由 saved_regressions_still_pass 与
    // saved_regressions_still_reproduce_their_original_failure 守着它。
    let dir = if std::env::var("IVMLITE_CAPTURE").as_deref() == Ok("1") {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions")
    } else {
        std::env::temp_dir().join("ivmlite-test-captured-regressions")
    };
    let path = save_regression(&dir, &minimal).expect("固化回归用例失败");
    eprintln!("已固化最小用例: {}", path.display());
}

/// C1 的核心断言：已提交的 fixture 不只是能反序列化，它必须仍能复现它当初
/// 被捕获时的那个失败——对 NoRetractionEngine 重放仍然 `Err`。
///
/// 这与 `saved_regressions_still_pass`（对 NaiveRecompute 重放、期望 `Ok`）
/// 证明的是两件不同的事：那条测的是"参照实现在回归用例上仍然平凡正确"；
/// 这条测的是"回归用例仍然是一个真实的失败见证，没有在 shrinker 行为变化
/// 后被静默替换成别的东西"。
#[test]
fn saved_regressions_still_reproduce_their_original_failure() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions");
    let cases = load_regressions(&dir).expect("读取回归用例目录失败");
    assert!(
        !cases.is_empty(),
        "回归目录不应为空——至少应含 seed0-ops1.json"
    );
    for case in cases {
        let mut engine = NoRetractionEngine::new();
        assert!(
            run(&mut engine, &case).is_err(),
            "回归用例 seed={} 必须仍能让 NoRetractionEngine 失败，否则这条 fixture 已经失效",
            case.seed
        );
    }
}

/// 本 Phase 的交付判据：框架能表达多表用例。
/// 查询仍是单表聚合（join 在引擎计划的 Phase 3），但两张表都在接收变更，
/// 所以下面三条代码路径都会被真正执行：apply 的表名路由、`NaiveRecompute`
/// 按表持有的 base/pending 存储、oracle 对 `Database` 里每张表的建表/插入。
///
/// "被执行到"不等于"这个测试会抓到它坏了"——三条里只有第一条是：
/// - apply 的表名路由：**会**。把 `run` 里递给 `apply` 的表名写死成
///   `db.tables()[0].table`，本测试会在 `diff[0]` 处变红（评审已实测确认，
///   见 `docs/mutation-gates.md` 里 M1a Phase 1 Task 5 那一行）。
/// - `NaiveRecompute` 按表持有的 base/pending：**不会**。Phase 1 的查询与
///   oracle 都只渲染 anchor 表（`db.tables()[0]`）的单表 SQL，非 anchor
///   表存进去的状态在 `materialize()` 和 oracle 比对里都不可观察——静默
///   丢弃它也不会让本测试变红。这是一个已登记的已知缺口，见
///   `docs/mutation-gates.md`「§8.5 `apply` 必须真的保留非 anchor 表的
///   delta」那一行；join 落地、oracle 开始渲染多表查询之后需要重新验证。
/// - oracle 对每张表的建表/插入：**不会**（这条由 `oracle::tests::
///   builds_every_table_in_the_database` 单独守护，不是本测试）。
#[test]
fn a_two_table_case_runs_green_against_the_reference_engine() {
    let db = gen_database(2);
    for seed in seed_range() {
        let case = gen_case(seed, &db, &Domain::default(), 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("参照实现不应失败: {f}"));
    }
}

/// I4：`shrink` 的 phase 3（逐表、逐行删初始数据）此前从未在真正的多表用例上
/// 跑过——评审用探针 `assert!(case.database.len() <= 1)` 证实了这一点。这里
/// 用一个两表 `Database` 找一个能让 `NoRetractionEngine` 失败的用例，喂给
/// `shrink`，然后断言两张表各自的初始行数都被真的缩小过，而不只是恰好在
/// 循环第一次迭代碰到的那张表。
///
/// 这条用例里查询仍然只读 anchor 表（`t0`）——查询渲染固定用 anchor，是
/// Phase 1 的既有限制——所以 `t1` 对 oracle 比对完全不可观察，正确的 shrink
/// 应当把它整个缩到 0 行。这正是本测试用来分辨"phase 3 处理了每一张表"与
/// "phase 3 只处理了第一张表"的信号：如果循环只处理第一张表（或压根没走到
/// `t1`），`t1` 会原样留着 25 行初始数据。
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
        .expect("应当至少有一个两表失败用例");

    // I4 的探针：phase 3 的逐表循环必须真的在一个 initial 里有多张表的用例上
    // 运行到，不能只是"签名接受多表、实际从未被这样调用过"。
    assert_eq!(
        case.database.len(),
        2,
        "这条测试必须喂给 shrink 一个真正的两表用例"
    );
    assert!(
        case.initial.keys().count() > 1,
        "phase 3 循环遍历的 best.initial.keys() 在起点就必须有一张以上的表"
    );

    let minimal = shrink(&case, NoRetractionEngine::new);

    // shrinker 的合法性门禁（spec §9.3）在多表路径上必须仍然成立。
    assert!(
        is_legal(&minimal.initial, &minimal.ops),
        "shrink 的产出必须始终合法：{minimal:?}"
    );

    let mut engine = NoRetractionEngine::new();
    let failure = run(&mut engine, &minimal).expect_err("缩小后必须仍然失败");
    // 与 failing_case_shrinks_to_under_ten_ops 同一个理由：is_legal 若被打穿，
    // 这个断言是唯一能把"伪产物"和"原始 bug 的同族失败"区分开的地方——在
    // 多表路径上这条门禁尤其容易被写错（用 A 表的行给 B 表的删改当合法性
    // 依据），所以这里也要求它真的挡住。
    assert!(
        !failure.stage.starts_with("oracle["),
        "缩小后的用例在 stage={} 失败——这是合法性门禁损坏时会收敛到的伪产物形态",
        failure.stage
    );

    let rows_by_table: BTreeMap<&str, usize> = minimal
        .initial
        .iter()
        .map(|(t, rows)| (t.as_str(), rows.len()))
        .collect();
    assert!(
        rows_by_table.values().all(|&n| n < 25),
        "phase 3 必须真的对每一张表都做过逐行删减，不能有表原封不动留着全部 25 行初始数据：{rows_by_table:?}"
    );
    let total_rows: usize = rows_by_table.values().sum();
    assert!(
        total_rows <= 10,
        "两张表加总的初始行数应当收敛到个位数——查询只读 anchor 表 t0，\
         非 anchor 的 t1 对 oracle 比对完全不可观察，正确的 shrink 应当把它\
         整个缩到 0 行；实得 {rows_by_table:?}"
    );
}

/// M1a 检查点（spec §11）：差分框架第一次在真实增量引擎上跑绿。
///
/// 这条测试与 `naive_engine_is_green_across_many_seeds` 的结构相同，
/// 但被测对象换成了 `IncrementalEngine`——它是增量的，而参照实现是全量
/// 重算，两者在每个 refresh 点都要与 oracle 一致。
#[test]
fn incremental_engine_is_green_across_the_enumerated_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate(&db.tables()[0]);
    let mut checked = 0usize;
    for seed in seed_range() {
        let query = &queries[seed as usize % queries.len()];
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
            panic!("增量引擎在 seed={seed} 上与 oracle 不一致：{f}");
        }
        checked += 1;
    }
    assert!(checked >= 50, "至少要跑过 50 个 seed，实跑 {checked}");
}

/// 增量引擎必须与全量重算在**每个 refresh 点**都一致，而不只是最终状态。
/// TransientDriftEngine 的存在就是为了证明这两者不是一回事（spec §9.1）。
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
    // `IncrementalEngine::materialize` 的固有方法返回裸 `ZSet`（签名见
    // engine.rs），会遮蔽同名的 `Engine::materialize`（返回
    // `Result<ZSet, EngineError>`）——固有方法总是优先于 trait 方法。这里
    // 要的是能 `.unwrap()` 的那个，所以用 UFCS 显式点名 trait 方法。
    assert_eq!(
        Engine::materialize(&mut inc).unwrap(),
        naive.materialize().unwrap(),
        "bootstrap 即不一致"
    );

    for batch in case.batches() {
        for (table, raw) in &batch {
            inc.apply(table, raw).unwrap();
            naive.apply(table, raw).unwrap();
        }
        inc.refresh().unwrap();
        naive.refresh().unwrap();
        assert_eq!(
            Engine::materialize(&mut inc).unwrap(),
            naive.materialize().unwrap(),
            "增量与全量重算在某个 refresh 点分叉"
        );
    }
}

/// spec §5.2 的边界校验必须在 create_view 处生效，而不是等到 refresh 时 panic。
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
    };
    let mut engine = IncrementalEngine::new();
    let err = engine
        .create_view(
            &db,
            &bad,
            &BTreeMap::from([(db.tables()[0].table.clone(), ZSet::new())]),
        )
        .expect_err("空 group_by 必须在 create_view 处被拒绝");
    assert!(
        err.0.contains("GROUP BY") || err.0.contains("group_by"),
        "错误应指名 group_by：{}",
        err.0
    );
}
