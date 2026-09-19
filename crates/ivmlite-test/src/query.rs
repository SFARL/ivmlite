use crate::{ColumnType, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AggFn {
    Count,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Agg {
    pub func: AggFn,
    /// COUNT(*) 为 None；SUM 必须为 Some，且指向 INTEGER 列。
    pub column: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Predicate {
    None,
    IntGt { column: usize, value: i64 },
    IsNotNull { column: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ViewQuery {
    /// v0 只允许裸列作为 group-by 键，不允许表达式（spec §7.1）。
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
}

impl ViewQuery {
    pub fn output_arity(&self) -> usize {
        self.group_by.len() + self.aggs.len()
    }

    pub fn to_sql(&self, schema: &Schema) -> String {
        let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

        let mut select: Vec<String> = self.group_by.iter().map(|i| name(*i)).collect();
        for agg in &self.aggs {
            select.push(match (agg.func, agg.column) {
                (AggFn::Count, _) => "COUNT(*)".to_string(),
                (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
                // item 8（deferred minor）：`column` 是 pub 字段，调用方可以直接
                // 构造出 `Agg { func: Sum, column: None }`——这条路径经由公开
                // API 可达，`unreachable!` 宣称了一个这个类型系统根本不保证的
                // 前提。用 `panic!` 如实标注"这是一个被拒绝的非法状态"。
                (AggFn::Sum, None) => panic!("SUM 必须指定列"),
            });
        }

        let where_clause = match &self.predicate {
            Predicate::None => String::new(),
            Predicate::IntGt { column, value } => {
                format!(" WHERE {} > {}", name(*column), value)
            }
            Predicate::IsNotNull { column } => {
                format!(" WHERE {} IS NOT NULL", name(*column))
            }
        };

        let group = self
            .group_by
            .iter()
            .map(|i| name(*i))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "SELECT {} FROM \"{}\"{} GROUP BY {}",
            select.join(", "),
            schema.table,
            where_clause,
            group
        )
    }
}

/// 穷举 v0 的 query 空间。
///
/// spec §9.2：v0 的组合数有限，穷举优于随机——可复现且覆盖完全。
/// 随机性留给更新序列。
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    let int_cols: Vec<usize> = (0..schema.arity())
        .filter(|i| schema.columns[*i].ty == ColumnType::Integer)
        .collect();

    let mut group_by_choices: Vec<Vec<usize>> = Vec::new();
    for i in 0..schema.arity() {
        group_by_choices.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            group_by_choices.push(vec![i, j]);
        }
    }

    let mut agg_choices: Vec<Vec<Agg>> = vec![vec![Agg {
        func: AggFn::Count,
        column: None,
    }]];
    for i in &int_cols {
        agg_choices.push(vec![Agg {
            func: AggFn::Sum,
            column: Some(*i),
        }]);
        agg_choices.push(vec![
            Agg {
                func: AggFn::Sum,
                column: Some(*i),
            },
            Agg {
                func: AggFn::Count,
                column: None,
            },
        ]);
    }

    let mut predicates = vec![Predicate::None];
    for i in &int_cols {
        predicates.push(Predicate::IntGt {
            column: *i,
            value: 4,
        });
    }
    for i in 0..schema.arity() {
        if schema.columns[i].nullable {
            predicates.push(Predicate::IsNotNull { column: i });
        }
    }

    let mut out = Vec::new();
    for group_by in &group_by_choices {
        for aggs in &agg_choices {
            for predicate in &predicates {
                out.push(ViewQuery {
                    group_by: group_by.clone(),
                    aggs: aggs.clone(),
                    predicate: predicate.clone(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};

    fn orders() -> Schema {
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
    fn to_sql_renders_group_by_and_aggs() {
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
        assert_eq!(
            q.to_sql(&orders()),
            "SELECT \"region\", SUM(\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"region\""
        );
    }

    #[test]
    fn to_sql_renders_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IntGt {
                column: 1,
                value: 3,
            },
        };
        assert!(q.to_sql(&orders()).contains("WHERE \"amount\" > 3"));
    }

    /// item 13（deferred minor，与 I4 同源）：`IsNotNull` 此前完全没有单元
    /// 测试覆盖——`enumerate` 里生成它的整段循环删掉之后 67 个测试照样全绿,
    /// 正是因为连 `to_sql` 这一层都没人断言过它的输出。
    #[test]
    fn to_sql_renders_is_not_null_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
        };
        assert!(
            q.to_sql(&orders()).contains("WHERE \"region\" IS NOT NULL"),
            "{}",
            q.to_sql(&orders())
        );
    }

    #[test]
    fn output_arity_is_group_by_plus_aggs() {
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
        assert_eq!(q.output_arity(), 3);
    }

    #[test]
    fn enumerate_covers_the_v0_space_and_is_nonempty() {
        let qs = enumerate(&orders());
        assert!(!qs.is_empty());
        assert!(
            qs.iter().all(|q| !q.group_by.is_empty()),
            "v0 禁止全局聚合：空表上 `SELECT SUM(v) FROM t` 返回 1 行 NULL，\
             而 `... GROUP BY g` 返回 0 行，两者不能共用同一套删行规则（spec §5.2）"
        );
        assert!(
            qs.iter().all(|q| !q.aggs.is_empty()),
            "v0 的根算子必须是 Aggregate——否则 __w 权重会让物化表与普通 SQL 视图行数不一致（spec §5.2）"
        );
        // I4：exhaustive match 而非三个 any() 调用——将来给 Predicate 加新
        // 变体时，这里会编译失败，逼着补上对应的覆盖断言，而不是像
        // IsNotNull 那样悄悄漏掉一整个分支还能全绿。
        let mut saw_none = false;
        let mut saw_int_gt = false;
        let mut saw_is_not_null = false;
        for q in &qs {
            match &q.predicate {
                Predicate::None => saw_none = true,
                Predicate::IntGt { .. } => saw_int_gt = true,
                Predicate::IsNotNull { .. } => saw_is_not_null = true,
            }
        }
        assert!(saw_none, "enumerate 必须产出 Predicate::None");
        assert!(saw_int_gt, "enumerate 必须产出 Predicate::IntGt");
        assert!(saw_is_not_null, "enumerate 必须产出 Predicate::IsNotNull");
    }

    #[test]
    fn enumerate_only_sums_integer_columns() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Sum {
                    let idx = agg.column.expect("SUM 必须有列");
                    assert_eq!(
                        schema.columns[idx].ty,
                        ColumnType::Integer,
                        "v0 无浮点，SUM 只能作用于 INTEGER 列"
                    );
                }
            }
        }
    }

    #[test]
    fn enumerate_always_count_has_none() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Count {
                    assert!(
                        agg.column.is_none(),
                        "COUNT(*) 必须有 column: None（spec §5.2）"
                    );
                }
            }
        }
    }
}
