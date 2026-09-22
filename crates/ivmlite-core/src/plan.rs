use crate::{Agg, Predicate, ViewQuery};

/// spec §5.2 的 plan IR。
///
/// v0 的合法形状恒为 `Scan → Filter? → Project → Aggregate`，由 `lower` 保证。
/// `Join` 变体留到 Phase 3 与 join 算子一起加——现在加一个所有 match 分支都
/// 只能写 `unreachable!()` 的变体，等于在每处匹配上留一段没有测试能到达的代码。
///
/// spec §5.2 把节点内的表达式写作 `Expr`，本实现用 `Vec<usize>`（列下标）与
/// `Predicate` 代替：v0 的 group-by key 只能是裸列（§5.2），谓词白名单（§6.1）
/// 也没有任何需要表达式树的形式，于是 `Expr` 在 v0 会是只有 `Column(usize)`
/// 一个变体的空壳。第一个真需要表达式的特性出现时再引入它是局部改动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Scan {
        table: String,
        columns: Vec<usize>,
    },
    Filter {
        input: Box<Plan>,
        predicate: Predicate,
    },
    Project {
        input: Box<Plan>,
        /// 要保留的**输入**列下标，按输出顺序排列。
        columns: Vec<usize>,
    },
    Aggregate {
        input: Box<Plan>,
        /// 下标相对于 `Project` 的**输出**，不是基表。
        group_by: Vec<usize>,
        /// 各 `Agg::column` 同样已重映射到 `Project` 的输出位置。
        aggs: Vec<Agg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PlanError {}

/// 把 harness 的扁平 `ViewQuery` 降成算子树，并在边界上执行 §5.2 的合法性校验。
///
/// 这是 `ViewQuery` 第一次跨越到 `enumerate` 之外的入口——引擎直接消费它。
/// `enumerate` 从不产出非法形状（`enumerate_covers_the_v0_space_and_is_nonempty`
/// 守着这一点），但 `ViewQuery` 本身可以自由构造，所以校验必须在这里。
///
/// `arity` 是基表的列数，用于下标越界检查。
pub fn lower(query: &ViewQuery, table: &str, arity: usize) -> Result<Plan, PlanError> {
    if query.group_by.is_empty() {
        return Err(PlanError(
            "spec §5.2：视图的根算子必须是带非空 GROUP BY 的 Aggregate；\
             禁止全局聚合（空表时它返回 1 行而分组聚合返回 0 行，\
             「组内计数归零就删行」这条规则对前者是错的）"
                .into(),
        ));
    }
    if query.aggs.is_empty() {
        return Err(PlanError(
            "spec §5.2：没有任何 agg 的查询实际是 Scan→Project 直接成为视图，\
             而 Z-set 权重与 SQL 行数在该形状下语义不一致"
                .into(),
        ));
    }

    let check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} 引用了列下标 {c}，但表 {table} 只有 {arity} 列"
            )))
        } else {
            Ok(())
        }
    };
    for &c in &query.group_by {
        check(c, "group_by")?;
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            check(c, "agg")?;
        }
    }
    match &query.predicate {
        Predicate::None => {}
        Predicate::IntGt { column, .. } | Predicate::IsNotNull { column } => {
            check(*column, "predicate")?
        }
    }

    // 投影保留的列：先 group key（按原序），再各 agg 的列（按原序），去重。
    // 顺序必须确定，否则 Aggregate 的下标重映射无从对齐（spec §9.4）。
    let mut keep: Vec<usize> = Vec::new();
    for &c in &query.group_by {
        if !keep.contains(&c) {
            keep.push(c);
        }
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            if !keep.contains(&c) {
                keep.push(c);
            }
        }
    }
    let remap = |c: usize| {
        keep.iter()
            .position(|&k| k == c)
            .expect("keep 由 group_by 与 agg 列构造，必然包含它们")
    };

    // Scan 取全部列：Filter 的谓词按**基表**下标求值，收窄发生在 Filter 之后。
    let mut node = Plan::Scan {
        table: table.to_string(),
        columns: (0..arity).collect(),
    };
    if query.predicate != Predicate::None {
        node = Plan::Filter {
            input: Box::new(node),
            predicate: query.predicate.clone(),
        };
    }
    node = Plan::Project {
        input: Box::new(node),
        columns: keep.clone(),
    };
    Ok(Plan::Aggregate {
        input: Box::new(node),
        group_by: query.group_by.iter().map(|&c| remap(c)).collect(),
        aggs: query
            .aggs
            .iter()
            .map(|a| Agg {
                func: a.func,
                column: a.column.map(remap),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery {
            group_by,
            aggs,
            predicate,
        }
    }

    fn count() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    fn sum(column: usize) -> Agg {
        Agg {
            func: AggFn::Sum,
            column: Some(column),
        }
    }

    #[test]
    fn lowers_to_scan_filter_project_aggregate() {
        // predicate 用列 1，聚合只要列 0——Project 必须把 2 列收窄成 1 列，
        // 且 Aggregate 的 group_by 下标必须重映射到收窄后的位置。
        let plan = lower(
            &q(
                vec![0],
                vec![count()],
                Predicate::IntGt {
                    column: 1,
                    value: 3,
                },
            ),
            "orders",
            2,
        )
        .expect("合法查询必须能降下来");

        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("根算子必须是 Aggregate（spec §5.2）：{plan:?}");
        };
        assert_eq!(group_by, &vec![0], "收窄后 group key 落在位置 0");
        assert_eq!(aggs.len(), 1);

        let Plan::Project { input, columns } = &**input else {
            panic!("Aggregate 之下必须是 Project：{input:?}");
        };
        assert_eq!(columns, &vec![0], "只有列 0 被聚合用到");

        let Plan::Filter { input, predicate } = &**input else {
            panic!("Project 之下必须是 Filter：{input:?}");
        };
        assert_eq!(
            predicate,
            &Predicate::IntGt {
                column: 1,
                value: 3
            }
        );

        let Plan::Scan { table, columns } = &**input else {
            panic!("最底层必须是 Scan：{input:?}");
        };
        assert_eq!(table, "orders");
        assert_eq!(columns, &vec![0, 1], "Scan 取全部列——Filter 按原始下标求值");
    }

    #[test]
    fn no_filter_node_when_predicate_is_none() {
        // Predicate::None 不应产生一个恒真的 Filter 节点：多一个节点就多一处
        // 每批都要走的无谓遍历，且会让「Filter 被正确跳过」这件事不可观察。
        let plan = lower(&q(vec![0], vec![count()], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, .. } = &plan else {
            panic!("{plan:?}")
        };
        let Plan::Project { input, .. } = &**input else {
            panic!("{input:?}")
        };
        assert!(
            matches!(&**input, Plan::Scan { .. }),
            "Predicate::None 之下应直接是 Scan，不得插入恒真 Filter：{input:?}"
        );
    }

    #[test]
    fn projection_keeps_group_keys_and_summed_columns_in_a_stable_order() {
        // group key 是列 1，SUM 的是列 0——收窄后的顺序必须确定且可预测，
        // 否则 Aggregate 的下标重映射无从对齐（spec §9.4）。
        let plan = lower(&q(vec![1], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("{plan:?}")
        };
        let Plan::Project { columns, .. } = &**input else {
            panic!("{input:?}")
        };
        assert_eq!(
            columns,
            &vec![1, 0],
            "先 group key（按原序），再各 agg 的列（按原序）"
        );
        assert_eq!(group_by, &vec![0], "group key 重映射到收窄后的位置 0");
        assert_eq!(aggs[0].column, Some(1), "SUM 的列重映射到收窄后的位置 1");
    }

    #[test]
    fn a_column_used_as_both_group_key_and_sum_target_is_projected_once() {
        // 同一列既当 group key 又被 SUM 时不得在投影里出现两次——出现两次
        // 会让 Project 的输出行宽与 Aggregate 的预期不一致。
        let plan = lower(&q(vec![0], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate {
            input,
            group_by,
            aggs,
        } = &plan
        else {
            panic!("{plan:?}")
        };
        let Plan::Project { columns, .. } = &**input else {
            panic!("{input:?}")
        };
        assert_eq!(columns, &vec![0], "去重后只投影一次");
        assert_eq!(group_by, &vec![0]);
        assert_eq!(aggs[0].column, Some(0));
    }

    #[test]
    fn empty_group_by_is_rejected_at_the_boundary() {
        // spec §5.2：禁止全局聚合——空表时它返回 1 行（值为 NULL），而分组
        // 聚合返回 0 行，「组内计数归零就删行」这条规则对前者是错的。
        // 此前这条只在生成器侧成立（enumerate 从不产出这种形状）；引擎直接
        // 消费 ViewQuery 之后，边界校验必须在这里。
        let err = lower(&q(vec![], vec![count()], Predicate::None), "orders", 2)
            .expect_err("空 group_by 必须被拒绝");
        assert!(
            err.0.contains("group_by") || err.0.contains("GROUP BY"),
            "错误信息应指名是 group_by 的问题：{}",
            err.0
        );
    }

    #[test]
    fn empty_aggs_is_rejected_at_the_boundary() {
        // 根算子必须是 Aggregate；没有任何聚合的 "Aggregate" 实际是
        // Scan→Project 直接成为视图，而那正是 §5.2 判为非法的形状
        // （Z-set 权重 2 会显示成 2 行，普通 SQL 视图显示 3 行）。
        let err = lower(&q(vec![0], vec![], Predicate::None), "orders", 2)
            .expect_err("空 aggs 必须被拒绝");
        assert!(
            err.0.contains("agg"),
            "错误信息应指名是 aggs 的问题：{}",
            err.0
        );
    }

    #[test]
    fn out_of_range_column_is_rejected() {
        // 下标越界必须在降的时候就报错，而不是等到 refresh 时 panic——
        // 引擎在 create_view 之后不应再有可预见的 panic 路径。
        let err = lower(&q(vec![7], vec![count()], Predicate::None), "orders", 2)
            .expect_err("越界 group key 必须被拒绝");
        assert!(err.0.contains('7'), "错误信息应指出越界的下标：{}", err.0);
    }
}
