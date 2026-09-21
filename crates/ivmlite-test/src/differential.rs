use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate, gen_initial, gen_ops, recompute_via_sqlite, view_query_to_sql,
    Domain, Engine, Op, ViewQuery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Batching {
    /// 全部 delta 一次性应用
    All,
    /// 每条 delta 单独应用
    One,
    /// 每 n 条一批
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
             重放本用例: IVMLITE_SEED={seed} cargo test -p ivmlite-test --test harness_catches_bugs -- --nocapture",
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
            Err(_) => panic!("IVMLITE_SEED 必须是 u64，实得 {s:?}"),
        },
        None => (0..50).collect(),
    }
}

/// 集成测试遍历的 seed 范围。设置 `IVMLITE_SEED` 时只跑那一个 seed——
/// 这就是 Failure 里那行重放命令生效的机制（spec §9.4）。
pub fn seed_range() -> Vec<u64> {
    parse_seed_arg(std::env::var("IVMLITE_SEED").ok())
}

/// 生成一个差分用例。`db` 声明用例涉及的全部基表（顺序确定，spec §9.4）；
/// 查询目前仍是单表聚合——渲染与枚举都固定用 `db.tables()[0]`（anchor 表），
/// join 查询的渲染属引擎计划（Phase 3），Task 3 的 oracle 已经把这条限制
/// 写死在 `recompute_via_sqlite` 里，这里跟随保持一致。
pub fn gen_case(
    seed: u64,
    db: &Database,
    domain: &Domain,
    rows_per_table: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    let mut rng = StdRng::seed_from_u64(seed);
    let initial = gen_initial(&mut rng, db, domain, rows_per_table);
    let ops = gen_ops(&mut rng, db, domain, &initial, op_count);
    let anchor = db.tables().first().expect("database 不应为空");
    let queries = enumerate(anchor);
    let query = queries[seed as usize % queries.len()].clone();
    TestCase {
        seed,
        database: db.clone(),
        query,
        initial,
        ops,
        batching,
    }
}

/// 把 ops 切成批次，每批按表分组成**未合并**的原始 `(Row, i64)` 序列——同一行
/// 在同一批同一张表里可以出现多次，是否 consolidate 交给引擎的 `apply` 决定
/// （spec §8.2）。harness 自己不做任何折叠：这正是 M1 的 consolidation 必须
/// 真正落地才能通过测试的原因。分组用 `BTreeMap` 保证按表迭代顺序确定
/// （spec §9.4），组内顺序沿用原始 op 序列顺序。
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

/// 跑完一个用例：逐批应用 delta，**每一个可观察的 refresh 点**都检查不变量
/// 并与 oracle 严格比对。
///
/// 为什么不能只比最终状态（spec §9.1）：一个"中途算错、形式上仍合法、后续
/// 又自行恢复"的实现可以完全通过末尾比对——而这正是状态漂移类 bug 的典型
/// 形态。不变量层拦不住它，因为错误的值同样满足"权重为 1、group key 唯一"。
///
/// 代价是复杂度从 O(n) 变成 O(n × 基表规模)，因此差分测试的用例规模必须
/// 保持很小（默认 25 行初始数据、150 步操作）。大规模场景交给 benchmark。
///
/// 每批只调一次 `refresh`，不是每张表一次（spec §8.2 「N 次 apply、一次
/// refresh」）：批内先把该批的 `(表名, Op)` 按表分组，对每张有变更的表各调
/// 一次 `apply`，随后统一调一次 `refresh`——这是 consolidation 唯一能发挥
/// 作用的地方。
pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure> {
    let fail = |stage: &str, detail: String| Failure {
        case_seed: case.seed,
        stage: stage.to_string(),
        detail,
    };

    let compare =
        |engine: &mut E, bases: &BTreeMap<String, ZSet>, stage: &str| -> Result<(), Failure> {
            let got = engine
                .materialize()
                .map_err(|e| fail(&format!("materialize[{stage}]"), e.to_string()))?;
            check_invariants(&got, &case.query)
                .map_err(|e| fail(&format!("invariants[{stage}]"), e))?;
            let want = recompute_via_sqlite(&case.database, &case.query, bases)
                .map_err(|e| fail(&format!("oracle[{stage}]"), e.to_string()))?;
            if got != want {
                let anchor = &case.database.tables()[0];
                return Err(fail(
                    &format!("diff[{stage}]"),
                    format!(
                        "引擎与 oracle 不一致\n  query: {}\n  引擎: {:?}\n  oracle: {:?}",
                        view_query_to_sql(&case.query, anchor),
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

    // bootstrap 之后立刻比对一次——空 ops 的用例也因此被真正检查到。
    compare(engine, &bases, "bootstrap")?;

    for (i, grouped) in batches(&case.ops, case.batching).into_iter().enumerate() {
        for (table, raw) in &grouped {
            engine
                .apply(table, raw)
                .map_err(|e| fail(&format!("apply[{i}][{table}]"), e.to_string()))?;
        }
        engine
            .refresh()
            .map_err(|e| fail(&format!("refresh[{i}]"), e.to_string()))?;
        // harness 自己的 reference bookkeeping 在这里合并——这是 harness 的业务，
        // 不是引擎的（spec §8.2）。引擎那边看到的仍然是每张表 `raw` 的原始形态。
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

/// spec §9.1 第二层：同一串 delta 无论怎么分批，最终状态必须一致。
/// 自动维护模式下无法测试这条性质，这是 v0 选择显式 refresh 的收益之一。
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
                            "{ref_mode:?} 与 {mode:?} 的最终状态不同\n  {ref_state:?}\n  {state:?}"
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
        gen_database, Agg, AggFn, Column, ColumnType, Domain, EngineError, NaiveRecompute,
        Predicate, Schema,
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

    // 与 naive.rs / buggy.rs / ops.rs / oracle.rs 里的单表 wrapper 同一个
    // 概念，但这里要构造的是整个 TestCase（含 query），形状不同，没有直接
    // 并进 test_support——`single_table_db` 是共用的那一半（m4）。
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

    /// 只做记录、不做别的：把真正的计算委托给 `NaiveRecompute`（保证 `run`
    /// 内部的 oracle 比对不会因为我们自己的引擎错误而失败），同时把每次
    /// `apply` 收到的 `(table, raw)` 原样存下来，供守卫测试断言。
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

    /// 守卫 1：同一行在同一批里出现两次，引擎必须原样收到两条 `(row, +1)`，
    /// 而不是 harness 替它合并成一条 `(row, +2)`。
    ///
    /// 破坏方式：让 `batches()`（或 `run` 里递给 `apply` 的那一步）重新把
    /// chunk 折进一个 `ZSet` 再展开——这个测试必须变红。
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
        run(&mut engine, &case).unwrap_or_else(|f| panic!("不应失败: {f}"));

        assert_eq!(
            engine.received.len(),
            1,
            "两条 op 用 Batching::All 应当落在同一批里"
        );
        let (_, raw) = &engine.received[0];
        let dup_entries = raw.iter().filter(|(row, w)| *row == dup && *w == 1).count();
        assert_eq!(
            dup_entries, 2,
            "同一行插入两次必须以两条独立的 (row, +1) 到达引擎，而不是合并成一条"
        );
    }

    /// 守卫 2：表名必须原样传到引擎——这是 join（M2）需要多张基表的前提。
    ///
    /// 破坏方式：在 `run` 里把 `apply` 的表名参数换成写死的常量或空字符串，
    /// 这个测试必须变红。
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
        run(&mut engine, &case).unwrap_or_else(|f| panic!("不应失败: {f}"));

        assert_eq!(engine.received.len(), 1);
        assert_eq!(
            engine.received[0].0,
            case.database.tables()[0].table,
            "apply 收到的表名必须等于用例声明的表名"
        );
    }

    /// 守卫 3：`refresh` 是 load-bearing 的——`apply` 之后不调用 `refresh`，
    /// `materialize` 必须仍然返回 apply 之前的状态；调用 `refresh` 之后才变化。
    ///
    /// 破坏方式：让 `NaiveRecompute::apply` 直接合并进 `base`（回到 M0 的行为），
    /// 这个测试必须变红——因为那样 `refresh` 就成了没有可观察效果的空操作。
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
            "apply 之后、refresh 之前，materialize 必须仍是 apply 前的状态"
        );

        engine.refresh().unwrap();
        let after = engine.materialize().unwrap();
        assert_ne!(
            after, before,
            "refresh 之后 materialize 必须反映刚才 apply 进来的变更"
        );
    }

    /// I3 的记录引擎：不止记下收到了什么，还按 `refresh` 分界把 `apply`
    /// 调用切成一个个批次——`(table, row_count)` 的序列，按批分组。
    /// `NaiveRecompute` 仍然是真正的计算委托对象，这里只加了记账。
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

    /// I3：钉死 `batches()` 用 `BTreeMap` 分组这件事的**唯一**可观察后果——
    /// 同一批内，`apply` 依次收到的表名必须严格按 `db.tables()` 的顺序递增
    /// （对 `gen_database` 产出的 `t0..t{n-1}` 命名，插入顺序与字典序恰好
    /// 重合，所以这条断言等价于「按字典序」，但写成对 `db.tables()`
    /// 的顺序断言更贴近 I3 的诉求：§7.2/§7.3 落地后，这个顺序要喂 GC 与
    /// bootstrap 水位，届时真正要保证的是「与 `db.tables()` 一致」，不是
    /// 「字典序恰好正确」这个巧合。
    ///
    /// 这条 guard 是**绝对的、非统计的**：只要 `batches()` 继续用 `BTreeMap`
    /// 分组，`Vec` 的迭代顺序在 `ivmlite-core` 侧已经被
    /// `table_order_is_preserved` 钉死，`BTreeMap` 的迭代顺序按 key 排序是
    /// 标准库文档承诺的行为，不依赖任何随机状态或运行时环境——同一份输入
    /// 每次跑都会得到同一个顺序，不存在"这次侥幸没抓到"的可能。
    ///
    /// 变异验证（见 I3 变异记录）：把 `batches()` 的返回类型与内部分组容器
    /// 都换成 `HashMap` 后，这条 guard 会变红，但那个方向的红是**统计的**——
    /// `HashMap` 的迭代顺序由每次构造时随机生成的 `RandomState` 决定，键
    /// 数量越少、巧合排对的概率越高，理论上不能排除某次运行偶然拿到正确
    /// 顺序。真正被这条测试钉死为绝对保证的，只有"继续用 `BTreeMap`"这一侧。
    #[test]
    fn per_batch_apply_order_follows_db_tables_order() {
        let db = gen_database(4);
        let domain = Domain::default();
        let case = gen_case(21, &db, &domain, 20, 150, Batching::Chunks(5));

        let mut engine = OrderRecordingEngine::default();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("不应失败: {f}"));

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
                    .unwrap_or_else(|| panic!("未知表 {table}"));
                if let Some(last) = last_idx {
                    assert!(
                        idx > last,
                        "批内表名顺序必须严格递增、匹配 db.tables()：{table_order:?} 中 \
                         上一张表下标 {last}，这次是 {idx}（表 {table}）"
                    );
                }
                last_idx = Some(idx);
            }
        }
        assert!(
            multi_table_batches > 0,
            "必须至少有一批真的涉及多张表，否则上面的顺序断言不会被执行到任何有意义的路径"
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
                panic!("seed {} 失败于 {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn batch_invariance_holds_for_naive_engine() {
        let db = Database::single(schema());
        let domain = Domain::default();
        let case = gen_case(4242, &db, &domain, 30, 200, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    /// I4：`check_batch_invariance` 此前从未在真正的多表用例上跑过——评审用
    /// 探针 `assert!(case.database.len() <= 1)` 证实了这一点。这里换成一个
    /// 两表 `Database`，让 `run`（`check_batch_invariance` 内部对每种
    /// `Batching` 都会调一次）真的走一遍多表的 apply 路由与 harness 侧
    /// `bases` bookkeeping。
    ///
    /// 老实说明这条测试目前能守住什么、不能守住什么：Phase 1 的查询与
    /// oracle 仍然只渲染 anchor 表（`db.tables()[0]`）的单表 SQL，所以
    /// 非 anchor 表的状态在 `materialize()` 里不可观察——这与 I2/§8.5
    /// 登记的两条"不适用"缺口是同一个根因。这条测试因此只钉死"多表用例
    /// 能跑通 `check_batch_invariance` 而不 panic/不报错"这件事本身，还
    /// 不能钉死"非 anchor 表的 delta 真的影响了批次无关性的结果"——后者要
    /// 等 join 落地、oracle 开始渲染多表查询之后才可能被任何断言区分开。
    #[test]
    fn batch_invariance_holds_for_naive_engine_on_a_two_table_case() {
        let db = gen_database(2);
        let domain = Domain::default();
        let case = gen_case(4343, &db, &domain, 30, 200, Batching::All);
        assert_eq!(case.database.len(), 2, "这条测试必须是真正的两表用例");
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
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
