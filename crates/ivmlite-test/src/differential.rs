use ivmlite_core::{Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate, gen_ops, gen_rows, recompute_via_sqlite, Domain, Engine, Op,
    Schema, ViewQuery,
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
    let ops = gen_ops(&mut rng, schema, domain, &initial, op_count);
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

fn batches(ops: &[Op], batching: Batching) -> Vec<ZSet> {
    let size = match batching {
        Batching::All => ops.len().max(1),
        Batching::One => 1,
        Batching::Chunks(n) => n.max(1),
    };
    ops.chunks(size)
        .map(|chunk| {
            let mut z = ZSet::new();
            for op in chunk {
                for (row, weight) in op.to_delta() {
                    z.update(row, weight);
                }
            }
            z
        })
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
        let want = recompute_via_sqlite(&case.schema, &case.query, base)
            .map_err(|e| fail(&format!("oracle[{stage}]"), e.to_string()))?;
        if got != want {
            return Err(fail(
                &format!("diff[{stage}]"),
                format!(
                    "引擎与 oracle 不一致\n  query: {}\n  引擎: {:?}\n  oracle: {:?}",
                    case.query.to_sql(&case.schema),
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

    for (i, delta) in batches(&case.ops, case.batching).into_iter().enumerate() {
        engine
            .apply(&delta)
            .map_err(|e| fail(&format!("apply[{i}]"), e.to_string()))?;
        base.merge(&delta);
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
    use crate::{Column, ColumnType, Domain, NaiveRecompute};

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
