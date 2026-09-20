use std::collections::BTreeMap;

use ivmlite_core::{Row, Value, ZSet};

use crate::{AggFn, Engine, EngineError, Predicate, Schema, ViewQuery};

/// 平凡正确的参照实现：保存全量基表，每次 materialize 重算一遍。
///
/// 两个用途：验证测试框架不会误报；充当 benchmark 的"朴素重跑"基线（spec §10.2）。
///
/// `apply` 与 `refresh` 是真正分离的两个阶段：`apply` 只把原始 `(Row, i64)`
/// 追加进 `pending`，不做任何合并；`refresh` 才把 `pending` drain 进
/// `base`。如果 `apply` 提前合并，`refresh` 就成了空操作，任何忽略 refresh
/// 契约的引擎都不会被测出来（spec §8.2）。
#[derive(Debug, Default)]
pub struct NaiveRecompute {
    query: Option<ViewQuery>,
    base: ZSet,
    pending: Vec<(Row, i64)>,
}

impl NaiveRecompute {
    pub fn new() -> Self {
        Self::default()
    }
}

fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(n) => n > value,
            _ => false, // NULL 与非整数一律不通过，与 SQL 的三值逻辑一致
        },
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}

impl Engine for NaiveRecompute {
    fn create_view(
        &mut self,
        _schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.query = Some(query.clone());
        self.base = initial.clone();
        Ok(())
    }

    fn apply(&mut self, _table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        // 刻意不合并：合并是 refresh 的职责，见类型上的文档注释。
        self.pending.extend_from_slice(raw);
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        for (row, weight) in self.pending.drain(..) {
            self.base.update(row, weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let query = self
            .query
            .as_ref()
            .ok_or_else(|| EngineError("materialize 前未 create_view".into()))?;

        // 每个聚合槽位是 (累加值, 非 NULL 输入的计数)。
        // 第二项是必须的：SUM 在非 NULL 输入为零行时返回 NULL 而非 0
        // （spec §6.1「聚合的 NULL 语义契约」）。只维护累加值会静默输出 0。
        let mut groups: BTreeMap<Vec<Value>, Vec<(i64, i64)>> = BTreeMap::new();

        for (row, weight) in self.base.iter() {
            if *weight <= 0 {
                continue;
            }
            if !passes(&query.predicate, row) {
                continue;
            }
            let key: Vec<Value> = query.group_by.iter().map(|i| row.get(*i).clone()).collect();
            let acc = groups
                .entry(key)
                .or_insert_with(|| vec![(0, 0); query.aggs.len()]);
            for (slot, agg) in acc.iter_mut().zip(&query.aggs) {
                match (agg.func, agg.column) {
                    (AggFn::Count, _) => slot.0 += weight,
                    (AggFn::Sum, Some(col)) => {
                        if let Value::Int(n) = row.get(col) {
                            slot.0 += n * weight;
                            slot.1 += weight;
                        }
                    }
                    (AggFn::Sum, None) => {
                        return Err(EngineError("SUM 缺少列".into()));
                    }
                }
            }
        }

        let mut out = ZSet::new();
        for (key, acc) in groups {
            let mut values = key;
            for (agg, (total, non_null)) in query.aggs.iter().zip(acc) {
                values.push(match agg.func {
                    // COUNT(*) 计的是行数，与列值是否 NULL 无关
                    AggFn::Count => Value::Int(total),
                    AggFn::Sum if non_null == 0 => Value::Null,
                    AggFn::Sum => Value::Int(total),
                });
            }
            out.update(Row::new(values), 1);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
    use ivmlite_core::{Row, Value};

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

    fn sum_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
        }
    }

    fn row(region: &str, amount: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(amount)])
    }

    fn out(region: Value, sum: i64, count: i64) -> Row {
        Row::new(vec![region, Value::Int(sum), Value::Int(count)])
    }

    #[test]
    fn aggregates_initial_state() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1), (row("b", 3), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 15, 2)), 1);
        assert_eq!(got.weight_of(&out(Value::Text("b".into()), 3, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn applying_a_delete_updates_the_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply("orders", &[(row("a", 5), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 10, 1)), 1);
        assert_eq!(got.len(), 1, "旧的 (a,15,2) 必须消失");
    }

    #[test]
    fn emptying_a_group_removes_it_entirely() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply("orders", &[(row("a", 10), -1)]).unwrap();
        e.refresh().unwrap();

        assert!(
            e.materialize().unwrap().is_empty(),
            "空 group 不得留下僵尸行"
        );
    }

    #[test]
    fn null_forms_its_own_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Null, Value::Int(4)]), 1),
            (row("a", 1), 1),
        ]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Null, 4, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn predicate_filters_before_aggregating() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IntGt {
                column: 1,
                value: 4,
            },
        };
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 1), 1)]);
        e.create_view(&schema(), &q, &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
    }

    /// item 13（deferred minor，与 I4 同源）：`IsNotNull` 谓词此前没有任何
    /// 行为覆盖——只有 `to_sql` 渲染，没有断言它真的把 NULL 行过滤掉。
    #[test]
    fn is_not_null_predicate_filters_out_null_rows() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
        };
        let base = ZSet::from_rows([
            (row("a", 10), 1),
            (Row::new(vec![Value::Null, Value::Int(1)]), 1),
        ]);
        e.create_view(&schema(), &q, &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
        assert_eq!(
            got.len(),
            1,
            "NULL 分组必须被 IsNotNull 过滤掉，不应出现在输出中"
        );
    }

    /// spec §6.1 的 NULL 语义契约。注意这与"组为空"不同：
    /// 组非空（COUNT(*) 为正），但被求和的列全是 NULL，此时 SUM 为 NULL。
    #[test]
    fn sum_over_all_null_column_is_null_not_zero() {
        let nullable_amount = Schema {
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
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
        };
        // 两条相同的行会被 ZSet 合并成权重 2
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
        ]);

        let mut e = NaiveRecompute::new();
        e.create_view(&nullable_amount, &q, &base).unwrap();
        let got = e.materialize().unwrap();

        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SUM 无非 NULL 输入时应为 NULL，COUNT(*) 仍为 2"
        );
    }

    /// 与上一个测试互补：这里非 NULL 输入存在（非零），只是它们求和后恰好为 0。
    /// 若把发射分支的判据从 `non_null == 0` 误改成 `total == 0`，这个测试会失败,
    /// 而其余测试都不会。
    #[test]
    fn sum_that_totals_zero_is_int_zero_not_null() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", -10), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 0, 2)),
            1,
            "非 NULL 输入求和恰为 0 时应输出 Int(0)，而非 Null"
        );
    }

    /// 对从未插入过的行做纯撤回（apply 一个权重 -1 的 delta），
    /// 会把它留在 self.base 中权重为负。若 materialize 中的
    /// `weight <= 0` 守卫被删除，这条负权重行会被当成一条真实输入行聚合进去,
    /// 但现有测试都不会发现——两个"删除"测试都是先插入、后撤回到权重恰好为 0,
    /// 而 ZSet 会在权重归零时直接移除该行，materialize 根本不会遍历到它。
    #[test]
    fn retracting_a_row_that_was_never_inserted_is_a_noop() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        // "b" 从未出现在 base 中；这条撤回让它在 self.base 里权重为 -1。
        e.apply("orders", &[(row("b", 999), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 10, 1)),
            1,
            "未受影响的组必须保持不变"
        );
        assert_eq!(got.len(), 1, "负权重的幽灵行不得产生输出，也不得影响其他组");
    }
}
