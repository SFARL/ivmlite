use crate::{Plan, Predicate, Row, Value, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeError(pub String);

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NodeError {}

/// 带状态的算子树。由 `Plan` 建出，此后 `delta` 反复被调用。
///
/// 与 `Plan` 分开是因为 `Plan` 是纯描述（可比较、可打印、将来可从 SQL 重建），
/// 而算子要持有状态。spec §5.3 存 SQL 原文而非序列化 IR，正是靠这条分离。
#[derive(Debug)]
pub enum Node {
    Scan {
        table: String,
    },
    Filter {
        input: Box<Node>,
        predicate: Predicate,
    },
    Project {
        input: Box<Node>,
        columns: Vec<usize>,
    },
}

impl Node {
    pub fn build(plan: &Plan) -> Result<Node, NodeError> {
        match plan {
            Plan::Scan { table, .. } => Ok(Node::Scan {
                table: table.clone(),
            }),
            Plan::Filter { input, predicate } => Ok(Node::Filter {
                input: Box::new(Node::build(input)?),
                predicate: predicate.clone(),
            }),
            Plan::Project { input, columns } => Ok(Node::Project {
                input: Box::new(Node::build(input)?),
                columns: columns.clone(),
            }),
            Plan::Aggregate { .. } => {
                Err(NodeError("Aggregate 算子尚未实现（本计划 Task 4）".into()))
            }
        }
    }

    /// 把某张表的一批 delta 推过本节点，返回本节点输出的 delta。
    ///
    /// spec §6.1：线性算子满足 `Δ(f(R)) = f(ΔR)`，于是 Filter / Project 无状态，
    /// delta 直接穿过。
    pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        match self {
            Node::Scan { table: own } => {
                if own == table {
                    input.clone()
                } else {
                    ZSet::new()
                }
            }
            Node::Filter {
                input: child,
                predicate,
            } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    if passes(predicate, row) {
                        out.update(row.clone(), w);
                    }
                }
                out
            }
            Node::Project {
                input: child,
                columns,
            } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    // 收窄后可能与另一行重合——`ZSet::update` 累加权重并在
                    // 归零时删条目，正是需要的 Z-set 语义。
                    let narrowed = Row::new(columns.iter().map(|&c| row.get(c).clone()).collect());
                    out.update(narrowed, w);
                }
                out
            }
        }
    }
}

/// spec §6.1 的三值逻辑：NULL 求值为 UNKNOWN，该行不进入结果。
///
/// 返回 `bool` 而非三值枚举，是因为在**筛选**语义下「不通过」与「未知」
/// 合并为同一种处理。但**不得据此认为 `NOT p` 等价于 `!p`**——v0 的谓词
/// 白名单里没有 `NOT`，正是因为每加一个都要重新论证一次三值逻辑。
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(i) => i > value,
            // NULL > 3 是 UNKNOWN；Text > Int 在 v0 的枚举里不会出现
            // （enumerate_only_sums_integer_columns 之外，IntGt 只对 Integer 列生成）。
            _ => false,
        },
        Predicate::IsNotNull { column } => !matches!(row.get(*column), Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lower, Agg, AggFn, Predicate, Value, ViewQuery, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    #[test]
    fn scan_only_absorbs_its_own_table() {
        // 单表时这看似多余，但它正是 join 两侧各自只吸收自己那张表的
        // delta 的机制（Phase 3 不必改动 Scan）。
        let mut n = Node::build(&Plan::Scan {
            table: "orders".into(),
            columns: vec![0, 1],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]);
        assert_eq!(n.delta("orders", &d), d, "自己的表：原样穿过");
        assert_eq!(n.delta("customers", &d), ZSet::new(), "别人的表：空");
    }

    #[test]
    fn filter_passes_deltas_through_unchanged_for_matching_rows() {
        // spec §6.1：线性算子 Δ(f(R)) = f(ΔR)——delta 直接穿过，无状态。
        // 权重必须原样保留，包括负权重（撤回一行满足谓词的行，
        // 撤回动作本身也要穿过去）。
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            predicate: Predicate::IntGt {
                column: 0,
                value: 3,
            },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(5)]), 1), (row(vec![int(9)]), -2)]);
        assert_eq!(n.delta("t", &d), d);
    }

    #[test]
    fn filter_drops_rows_that_fail_the_predicate() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            predicate: Predicate::IntGt {
                column: 0,
                value: 3,
            },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1)]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn filter_treats_null_as_unknown_not_as_false_negation() {
        // spec §6.1 三值逻辑：v 取 {1, NULL, 5} 时 `v > 3` 命中 1 行，
        // `NOT (v > 3)` 也只命中 1 行——两者加起来是 2 而不是 3。
        // 这个测试钉的是 NULL 行两边都不进，而不是「NULL 等价于 false」。
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            predicate: Predicate::IntGt {
                column: 0,
                value: 3,
            },
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(1)]), 1),
            (row(vec![Value::Null]), 1),
            (row(vec![int(5)]), 1),
        ]);
        let got = n.delta("t", &d);
        assert_eq!(got, ZSet::from_rows([(row(vec![int(5)]), 1)]));
        assert_eq!(
            got.weight_of(&row(vec![Value::Null])),
            0,
            "NULL 行不得进入结果"
        );
    }

    #[test]
    fn is_not_null_predicate_filters_null_rows() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            predicate: Predicate::IsNotNull { column: 0 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![Value::Null]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn project_narrows_columns_and_preserves_weights() {
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0, 1, 2],
            }),
            columns: vec![2, 0],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2), int(3)]), 4)]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![int(3), int(1)]), 4)]),
            "列按 columns 给出的顺序重排，权重原样保留"
        );
    }

    #[test]
    fn project_merges_rows_that_become_identical_after_narrowing() {
        // 两行在收窄后变成同一行时，权重必须相加而不是后者覆盖前者——
        // 这是 Z-set 语义，也是 Project 唯一一处不平凡的地方。
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0, 1],
            }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), 3),
        ]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(7)]), 5)]));
    }

    #[test]
    fn project_drops_rows_whose_weights_cancel_after_narrowing() {
        // 收窄后权重相消为 0 的行必须消失（spec §5.1），不得留成权重 0 的条目。
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0, 1],
            }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), -2),
        ]);
        assert!(n.delta("t", &d).is_empty(), "相消后必须为空");
    }

    #[test]
    fn building_an_aggregate_is_an_error_until_task_4() {
        // 占位：Task 4 把这个测试删掉并换成真实的聚合测试。留它在这里是为了
        // 「未实现」有一个明确的、会被执行到的形态，而不是一个 panic。
        let err = Node::build(&Plan::Aggregate {
            input: Box::new(Plan::Scan {
                table: "t".into(),
                columns: vec![0],
            }),
            group_by: vec![0],
            aggs: vec![],
        })
        .expect_err("Task 3 尚未实现 Aggregate");
        assert!(err.0.contains("Aggregate"));
    }

    #[test]
    fn filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones() {
        // Task 1 遗留的债务（见 docs/mutation-gates.md 对应行）：`lower()`
        // 产出的 `Filter` 谓词必须按**基表**列下标求值，这只有在 `Filter`
        // 位于 `Project` **之前**（从而还能看到收窄前的宽行）时才成立。
        // `Plan` 在 Task 1 落地时还没有任何消费者，这条语义要求当时不可
        // 证伪。`Node` 是第一个消费者，这里补上——这个测试就是那笔债。
        //
        // 用 lower() 建出真实的树：group_by=[0]（只保留列 0），谓词读列 1
        // （IntGt{column:1,...}）——谓词列与投影列不同，用错下标（收窄后
        // 只剩列 0）要么会把列 0 的值错当成谓词输入，要么直接越界 panic。
        let query = ViewQuery {
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
        let plan = lower(&query, "t", 2).expect("合法查询必须能降下来");
        let Plan::Aggregate { input, .. } = plan else {
            panic!("lower 的根算子必须是 Aggregate");
        };
        // Task 3 还没有 Aggregate 节点（Task 4 才加），所以从 Aggregate 的
        // input——也就是 lower() 真实产出的 Filter/Project 子树——建 Node，
        // 而不是手写一棵形状相似的等价树。
        let mut n = Node::build(&input).unwrap();

        // 列 0 = group key（会被保留），列 1 = 谓词看的列（会被收窄掉）。
        let d = ZSet::from_rows([
            (row(vec![int(100), int(5)]), 1), // 列 1: 5 > 3 → 通过
            (row(vec![int(200), int(1)]), 1), // 列 1: 1 > 3 → 不通过
        ]);
        let got = n.delta("t", &d);
        assert_eq!(
            got,
            ZSet::from_rows([(row(vec![int(100)]), 1)]),
            "谓词必须按基表下标（列 1）求值；若按收窄后的下标求值，\
             收窄后的行只剩 1 列，取列 1 要么越界 panic，要么错误地把\
             列 0（group key）的值当成谓词输入"
        );
    }
}
