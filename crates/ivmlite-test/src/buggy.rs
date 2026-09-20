use ivmlite_core::{Row, Value, ZSet};

use crate::{Engine, EngineError, NaiveRecompute, Schema, ViewQuery};

/// 故意植入 bug 的引擎：聚合结果变化时**只发出新行、不撤回旧行**。
///
/// 这正是 spec §6.2 所说的 IVM 头号 bug 来源。它的存在不是为了被修好，
/// 而是为了证明测试框架抓得住它。
#[derive(Debug, Default)]
pub struct NoRetractionEngine {
    inner: NaiveRecompute,
    accumulated: ZSet,
}

impl NoRetractionEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Engine for NoRetractionEngine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.inner.create_view(schema, query, initial)?;
        self.accumulated = self.inner.materialize()?;
        Ok(())
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.inner.apply(delta)?;
        // BUG（有意为之）：把新的聚合结果并进来，却从不撤回上一次发出的行。
        let fresh = self.inner.materialize()?;
        for (row, weight) in fresh.iter() {
            self.accumulated.update(row.clone(), *weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        Ok(self.accumulated.clone())
    }
}

/// 在第 `drift_at` 次 `materialize` 返回一个被污染、但**形式合法**的状态，
/// 之后恢复正确。
///
/// 它只为证明一件事：逐批比对 oracle 抓得到「中途算错、形式合法、之后自愈」
/// 的实现，而只比最终状态抓不到。这是 spec §9.1 为逐批比对付出
/// O(n × 基表规模) 代价的**唯一证据**——没有这个反例，那笔开销就没有依据。
///
/// 污染方式刻意保持全部不变量成立：权重仍为 1、行宽不变、group key（前缀列）
/// 不变，所以 `check_invariants` 会放行。只有 oracle 比对能抓到它。
#[derive(Debug)]
pub struct TransientDriftEngine {
    inner: NaiveRecompute,
    calls: usize,
    drift_at: usize,
    /// group-by 键的列数：污染只允许发生在聚合列区间（`group_arity..`），
    /// 绝不能碰 group key，否则会改变污染行落在哪个 group。
    group_arity: usize,
}

impl TransientDriftEngine {
    /// `drift_at` 按 `materialize` 的调用序数计，从 1 开始。
    /// 在 `run` 中第 1 次是 bootstrap，第 2 次是第一批之后。
    pub fn new(drift_at: usize) -> Self {
        Self {
            inner: NaiveRecompute::new(),
            calls: 0,
            drift_at,
            group_arity: 0,
        }
    }
}

impl Engine for TransientDriftEngine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.group_arity = query.group_by.len();
        self.inner.create_view(schema, query, initial)
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.inner.apply(delta)
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        self.calls += 1;
        let truth = self.inner.materialize()?;
        if self.calls != self.drift_at {
            return Ok(truth);
        }
        // item 21（deferred minor）：污染第一行的聚合列区间（`group_arity..`,
        // group key 本身绝不碰）。找到该区间里第一个 Int 并 +1；若聚合列
        // 全是 Null（SUM 在零个非 NULL 输入下的语义），把最后一个聚合列从
        // Null 改成 Int(0)。两条路径都保证一定产生污染——不像之前那样只在
        // "首行末列恰好是 Int" 这个数据形状下才生效：若某个 seed 下首行末
        // 列恰为 Null 就完全不污染，而这个引擎存在的唯一理由（证明逐批
        // oracle 比对能抓到中途漂移，spec §9.1）会因此静默失效。
        let mut drifted = ZSet::new();
        for (i, (row, weight)) in truth.iter().enumerate() {
            let mut values = row.0.clone();
            if i == 0 {
                let agg_start = self.group_arity.min(values.len());
                let first_int =
                    (agg_start..values.len()).find(|&j| matches!(values[j], Value::Int(_)));
                match first_int {
                    Some(j) => {
                        if let Value::Int(n) = values[j] {
                            values[j] = Value::Int(n + 1);
                        }
                    }
                    None => {
                        if let Some(last) = values.len().checked_sub(1).filter(|&l| l >= agg_start)
                        {
                            values[last] = Value::Int(0);
                        }
                    }
                }
            }
            drifted.update(Row::new(values), *weight);
        }
        Ok(drifted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: false,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        }
    }

    fn sum_query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
            predicate: Predicate::None,
        }
    }

    /// item 21 的直接守卫：SUM 在零个非 NULL 输入下为 Null——旧实现只检查
    /// "末列是不是 Int"，这种情况下什么也不做，污染悄悄消失。新实现必须
    /// 把这个 Null 聚合列改成 Int(0)，让污染在这条路径上也一定发生。
    #[test]
    fn drift_still_happens_when_the_aggregate_column_is_null() {
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Null]), 1)]);
        let mut engine = TransientDriftEngine::new(1);
        engine.create_view(&schema(), &sum_query(), &base).unwrap();
        let truth = Row::new(vec![Value::Text("a".into()), Value::Null]);

        let drifted = engine.materialize().unwrap();
        assert_ne!(
            drifted.weight_of(&truth),
            1,
            "SUM 全 NULL 时污染必须仍然发生，不能因为末列是 Null 就放过"
        );
        assert_eq!(
            drifted.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(0)])),
            1,
            "全 Null 聚合列的污染约定是把它改成 Int(0)"
        );
    }

    /// 非 Null 情况保持原行为：找到聚合列区间里第一个 Int 并 +1。
    #[test]
    fn drift_bumps_the_first_int_aggregate_column() {
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Int(5)]), 1)]);
        let mut engine = TransientDriftEngine::new(1);
        engine.create_view(&schema(), &sum_query(), &base).unwrap();

        let drifted = engine.materialize().unwrap();
        assert_eq!(
            drifted.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(6)])),
            1
        );
    }
}
