use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate, gen_ops, gen_rows, recompute_via_sqlite, view_query_to_sql,
    Domain, Engine, Op, Schema, ViewQuery,
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
    pub schema: Schema,
    pub query: ViewQuery,
    pub initial: Vec<Row>,
    pub ops: Vec<Op>,
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

pub fn gen_case(
    seed: u64,
    schema: &Schema,
    domain: &Domain,
    initial_rows: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    let mut rng = StdRng::seed_from_u64(seed);
    let initial = gen_rows(&mut rng, schema, domain, initial_rows);
    // `gen_ops` 现在是多表签名（task 4）：单表用例包成一个只有这一张表
    // 的 `Database`，再把表标签剥掉还原成 `Vec<Op>`——`TestCase` 的序列化
    // 形状（含已落盘的回归用例 JSON）保持不变。
    let db = Database::single(schema.clone());
    let mut initial_map = BTreeMap::new();
    initial_map.insert(schema.table.clone(), initial.clone());
    let ops = gen_ops(&mut rng, &db, domain, &initial_map, op_count)
        .into_iter()
        .map(|(_, op)| op)
        .collect();
    let queries = enumerate(schema);
    let query = queries[seed as usize % queries.len()].clone();
    TestCase {
        seed,
        schema: schema.clone(),
        query,
        initial,
        ops,
        batching,
    }
}

/// 把 ops 切成批次，每批是**未合并**的原始 `(Row, i64)` 序列——同一行在同一批
/// 里可以出现多次，是否 consolidate 交给引擎的 `apply` 决定（spec §8.2）。
/// harness 自己不做任何折叠：这正是 M1 的 consolidation 必须真正落地才能
/// 通过测试的原因。
fn batches(ops: &[Op], batching: Batching) -> Vec<Vec<(Row, i64)>> {
    let size = match batching {
        Batching::All => ops.len().max(1),
        Batching::One => 1,
        Batching::Chunks(n) => n.max(1),
    };
    ops.chunks(size)
        .map(|chunk| chunk.iter().flat_map(Op::to_delta).collect())
        .collect()
}

fn initial_zset(initial: &[Row]) -> ZSet {
    ZSet::from_rows(initial.iter().cloned().map(|r| (r, 1)))
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
pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure> {
    let fail = |stage: &str, detail: String| Failure {
        case_seed: case.seed,
        stage: stage.to_string(),
        detail,
    };

    let compare = |engine: &mut E, base: &ZSet, stage: &str| -> Result<(), Failure> {
        let got = engine
            .materialize()
            .map_err(|e| fail(&format!("materialize[{stage}]"), e.to_string()))?;
        check_invariants(&got, &case.query)
            .map_err(|e| fail(&format!("invariants[{stage}]"), e))?;
        let db = Database::single(case.schema.clone());
        let bases = BTreeMap::from([(case.schema.table.clone(), base.clone())]);
        let want = recompute_via_sqlite(&db, &case.query, &bases)
            .map_err(|e| fail(&format!("oracle[{stage}]"), e.to_string()))?;
        if got != want {
            return Err(fail(
                &format!("diff[{stage}]"),
                format!(
                    "引擎与 oracle 不一致\n  query: {}\n  引擎: {:?}\n  oracle: {:?}",
                    view_query_to_sql(&case.query, &case.schema),
                    got,
                    want
                ),
            ));
        }
        Ok(())
    };

    let mut base = initial_zset(&case.initial);
    engine
        .create_view(&case.schema, &case.query, &base)
        .map_err(|e| fail("create_view", e.to_string()))?;

    // bootstrap 之后立刻比对一次——空 ops 的用例也因此被真正检查到。
    compare(engine, &base, "bootstrap")?;

    for (i, raw) in batches(&case.ops, case.batching).into_iter().enumerate() {
        engine
            .apply(&case.schema.table, &raw)
            .map_err(|e| fail(&format!("apply[{i}]"), e.to_string()))?;
        engine
            .refresh()
            .map_err(|e| fail(&format!("refresh[{i}]"), e.to_string()))?;
        // harness 自己的 reference bookkeeping 在这里合并——这是 harness 的业务，
        // 不是引擎的（spec §8.2）。引擎那边看到的仍然是 `raw` 的原始形态。
        for (row, weight) in &raw {
            base.update(row.clone(), *weight);
        }
        compare(engine, &base, &i.to_string())?;
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
    use crate::{Agg, AggFn, Column, ColumnType, Domain, EngineError, NaiveRecompute, Predicate};
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
            schema: &Schema,
            query: &ViewQuery,
            initial: &ZSet,
        ) -> Result<(), EngineError> {
            self.inner.create_view(schema, query, initial)
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
        let schema = schema();
        let query = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        };
        let dup = Row::new(vec![Value::Text("a".into()), Value::Int(1)]);
        let case = TestCase {
            seed: 0,
            schema,
            query,
            initial: vec![],
            ops: vec![Op::Insert(dup.clone()), Op::Insert(dup.clone())],
            batching: Batching::All,
        };

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
        let schema = schema();
        let query = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        };
        let case = TestCase {
            seed: 0,
            schema: schema.clone(),
            query,
            initial: vec![],
            ops: vec![Op::Insert(Row::new(vec![
                Value::Text("a".into()),
                Value::Int(1),
            ]))],
            batching: Batching::All,
        };

        let mut engine = RecordingEngine::default();
        run(&mut engine, &case).unwrap_or_else(|f| panic!("不应失败: {f}"));

        assert_eq!(engine.received.len(), 1);
        assert_eq!(
            engine.received[0].0, case.schema.table,
            "apply 收到的表名必须等于 case.schema.table"
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
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Int(1)]), 1)]);
        let mut engine = NaiveRecompute::new();
        engine.create_view(&schema, &query, &base).unwrap();
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

    #[test]
    fn naive_engine_passes_every_enumerated_query() {
        let schema = schema();
        let domain = Domain::default();
        for (i, query) in crate::enumerate(&schema).into_iter().enumerate() {
            let mut case = gen_case(i as u64, &schema, &domain, 30, 200, Batching::Chunks(7));
            case.query = query;
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} 失败于 {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn batch_invariance_holds_for_naive_engine() {
        let schema = schema();
        let domain = Domain::default();
        let case = gen_case(4242, &schema, &domain, 30, 200, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    #[test]
    fn same_seed_produces_the_same_case() {
        let schema = schema();
        let domain = Domain::default();
        let a = gen_case(5, &schema, &domain, 10, 40, Batching::One);
        let b = gen_case(5, &schema, &domain, 10, 40, Batching::One);
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
