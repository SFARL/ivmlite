use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Value, ZSet};

use crate::{Engine, EngineError, NaiveRecompute, ViewQuery};

/// An engine with a deliberately planted bug: when an aggregate result changes, it **emits the new row without retracting the old one**.
///
/// This is exactly what spec §6.2 calls IVM's number-one source of bugs. It
/// exists not to be fixed but to prove the test framework catches it.
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
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        self.inner.create_view(db, query, initial)?;
        self.accumulated = self.inner.materialize()?;
        Ok(())
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        self.inner.apply(table, raw)
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        self.inner.refresh()?;
        // BUG (deliberate): merge in the new aggregate result, but never retract the row emitted last time.
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

/// On the `drift_at`-th call to `materialize`, return a corrupted but
/// **well-formed** state, then go back to being correct.
///
/// It exists to prove one thing: comparing against the oracle per batch catches
/// an implementation that "goes wrong midway, stays well-formed, and later
/// heals itself", while comparing only the final state does not. It is **the
/// only evidence** for the O(n × base-table size) cost spec §9.1 pays for
/// per-batch comparison — without this counterexample, that cost would have no
/// justification.
///
/// The corruption deliberately keeps every invariant intact — weight still 1,
/// row width unchanged, group key (the prefix columns) unchanged — so
/// `check_invariants` lets it through. Only the oracle comparison can catch it.
#[derive(Debug)]
pub struct TransientDriftEngine {
    inner: NaiveRecompute,
    calls: usize,
    drift_at: usize,
    /// The number of group-by key columns: corruption may only happen in the
    /// aggregate columns (`group_arity..`) and must never touch the group key,
    /// or it would change which group the corrupted row belongs to.
    group_arity: usize,
}

impl TransientDriftEngine {
    /// `drift_at` counts calls to `materialize`, starting at 1. In `run` the
    /// first call is the bootstrap and the second comes after the first batch.
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
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        self.group_arity = query.group_by.len();
        self.inner.create_view(db, query, initial)
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        self.inner.apply(table, raw)
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        self.inner.refresh()
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        self.calls += 1;
        let truth = self.inner.materialize()?;
        if self.calls != self.drift_at {
            return Ok(truth);
        }
        // Item 21 (a deferred minor): corrupt the first row's aggregate columns
        // (`group_arity..`, never the group key itself). Find the first Int in
        // that range and add 1; if the aggregate columns are all Null (SUM's
        // semantics over zero non-NULL inputs), change the last aggregate
        // column from Null to Int(0). Both paths are guaranteed to corrupt —
        // unlike before, when it worked only for the data shape "the first
        // row's last column happens to be an Int": for a seed whose first row
        // ended in Null there was no corruption at all, and this engine's only
        // reason to exist (proving per-batch oracle comparison catches midway
        // drift, spec §9.1) would silently stop working.
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
    use crate::test_support::single_table_bases as single_table_case;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema};

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

    // The helper that was byte-identical to naive.rs's has been merged into test_support (m4).

    /// Item 21's direct guard: SUM over zero non-NULL inputs is Null — the old
    /// implementation only checked "is the last column an Int", did nothing in
    /// this case, and the corruption quietly vanished. The new one must change
    /// this Null aggregate column to Int(0), so corruption is certain on this
    /// path too.
    #[test]
    fn drift_still_happens_when_the_aggregate_column_is_null() {
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Null]), 1)]);
        let mut engine = TransientDriftEngine::new(1);
        let (db, bases) = single_table_case(&schema(), base);
        engine.create_view(&db, &sum_query(), &bases).unwrap();
        let truth = Row::new(vec![Value::Text("a".into()), Value::Null]);

        let drifted = engine.materialize().unwrap();
        assert_ne!(
            drifted.weight_of(&truth),
            1,
            "corruption must still happen when SUM is all NULL, not be skipped because the last column is Null"
        );
        assert_eq!(
            drifted.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(0)])),
            1,
            "the convention for corrupting an all-Null aggregate column is to change it to Int(0)"
        );
    }

    /// The non-Null case keeps the original behaviour: find the first Int among the aggregate columns and add 1.
    #[test]
    fn drift_bumps_the_first_int_aggregate_column() {
        let base = ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), Value::Int(5)]), 1)]);
        let mut engine = TransientDriftEngine::new(1);
        let (db, bases) = single_table_case(&schema(), base);
        engine.create_view(&db, &sum_query(), &bases).unwrap();

        let drifted = engine.materialize().unwrap();
        assert_eq!(
            drifted.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(6)])),
            1
        );
    }
}
