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
}

impl TransientDriftEngine {
    /// `drift_at` 按 `materialize` 的调用序数计，从 1 开始。
    /// 在 `run` 中第 1 次是 bootstrap，第 2 次是第一批之后。
    pub fn new(drift_at: usize) -> Self {
        Self {
            inner: NaiveRecompute::new(),
            calls: 0,
            drift_at,
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
        // 只把第一行最后一个聚合列 +1：行宽、group key、权重全部不变。
        let mut drifted = ZSet::new();
        for (i, (row, weight)) in truth.iter().enumerate() {
            let mut values = row.0.clone();
            if i == 0 {
                if let Some(Value::Int(n)) = values.last() {
                    let bumped = *n + 1;
                    *values.last_mut().expect("刚判断过非空") = Value::Int(bumped);
                }
            }
            drifted.update(Row::new(values), *weight);
        }
        Ok(drifted)
    }
}
