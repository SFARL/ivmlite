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
    Aggregate {
        input: Box<Node>,
        state: crate::AggState,
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
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => Ok(Node::Aggregate {
                input: Box::new(Node::build(input)?),
                state: crate::AggState::new(group_by.clone(), aggs.clone()),
            }),
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
            Node::Aggregate {
                input: child,
                state,
            } => {
                // spec §6.2：聚合是本引擎唯一的有状态算子。上游 delta 先算出来，
                // 再交给 `AggState` 去决定对外该撤回什么、发出什么。
                let upstream = child.delta(table, input);
                state.absorb(&upstream)
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
            // NULL > 3 是 UNKNOWN，`false` 是对的。
            //
            // Text > Int 的 `false` 则不是同一类保证：它在 v0 里从未被观察到，
            // 是因为 `crates/ivmlite-test/src/query.rs` 的 `enumerate` 只对
            // `int_cols` 里的列生成 `IntGt`（第 41-47 行），Text 列永远不会
            // 走到这个分支——不是因为 `false` 这个答案本身是对的。
            //
            // 它甚至是错的：SQLite 的类型排序是 NULL < INTEGER/REAL < TEXT
            // < BLOB，`'abc' > 3` 在 SQLite 里的真实答案是 `1`（true），不是
            // `0`。这里返回 `false` 与 oracle 相反，只是因为 v0 从不生成会
            // 触发这条分支的查询，所以从未被任何测试或差分比对拆穿——
            // `NaiveRecompute::passes`（`crates/ivmlite-test/src/naive.rs`
            // 第 37-40 行）有一模一样的折叠，差分层结构性地测不出来。
            // 见 docs/mutation-gates.md 对应「不适用」行。
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
    fn aggregate_can_be_built_and_runs_through_the_tree() {
        // 端到端：Scan → Filter → Project → Aggregate 整棵树推一批 delta。
        let plan = crate::lower(
            &crate::ViewQuery {
                group_by: vec![0],
                aggs: vec![crate::Agg {
                    func: crate::AggFn::Count,
                    column: None,
                }],
                predicate: Predicate::IntGt {
                    column: 1,
                    value: 3,
                },
            },
            "t",
            2,
        )
        .unwrap();
        let mut n = Node::build(&plan).unwrap();
        let d = ZSet::from_rows([
            (row(vec![Value::Text("a".into()), int(9)]), 1),
            (row(vec![Value::Text("a".into()), int(1)]), 1), // 被 Filter 挡掉
        ]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "只有通过谓词的那一行进入计数"
        );
    }

    #[test]
    fn aggregate_retracts_across_two_batches_through_the_same_node() {
        // **`AggState` 必须在两批之间存活。** 这与「delta 必须真的喂给
        // `AggState`」是两条互相独立的性质：`aggregate_can_be_built_and_runs_
        // through_the_tree` 只推一批，于是整个 retraction 协议通过 `Node`
        // 这一层根本没有被测到——实测：把 `Aggregate` 分支改成
        // `let mut scratch = state.clone(); scratch.absorb(&upstream)`
        // （并给 `AggState` 临时加回 `Clone`），全套仍然全绿。
        //
        // 这不是一个牵强的变异：spec §5.3 存的是 SQL 原文而不是序列化的 IR，
        // 所以「每次 refresh 重新 build 一棵树」是完全可能的重构；把 `delta`
        // 改成收 `&self` 也一样。任何一个都会把引擎里唯一的有状态算子变回
        // 无状态——每批只发 `+1`、永不撤回，而这正是 §6.2 的「最大的 bug 来源」。
        let plan = crate::lower(
            &crate::ViewQuery {
                group_by: vec![0],
                aggs: vec![crate::Agg {
                    func: crate::AggFn::Count,
                    column: None,
                }],
                predicate: Predicate::IntGt {
                    column: 1,
                    value: 3,
                },
            },
            "t",
            2,
        )
        .unwrap();
        let mut n = Node::build(&plan).unwrap();

        let first = n.delta(
            "t",
            &ZSet::from_rows([(row(vec![Value::Text("a".into()), int(9)]), 1)]),
        );
        assert_eq!(
            first,
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "第一批：组 a 首次出现，COUNT=1"
        );

        // 第二批推进**同一个** Node。组 a 的 COUNT 从 1 变 2，于是必须先撤回
        // 上一批发出的那行、再发新行——不是单独一行 +1。
        let second = n.delta(
            "t",
            &ZSet::from_rows([(row(vec![Value::Text("a".into()), int(5)]), 1)]),
        );
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![Value::Text("a".into()), int(1)]), -1),
                (row(vec![Value::Text("a".into()), int(2)]), 1),
            ]),
            "跨批必须撤回上一批发出的 COUNT=1 那行；只发 (a,2) w=+1 说明状态没有跨批存活"
        );
    }

    #[test]
    fn filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones() {
        // Task 1 遗留的债务（见 docs/mutation-gates.md 对应行）：`lower()`
        // 产出的 `Filter` 谓词必须按**基表**列下标求值，这只有在 `Filter`
        // 位于 `Project` **之前**（从而还能看到收窄前的宽行）时才成立。
        // `Plan` 在 Task 1 落地时还没有任何消费者，这条语义要求当时不可
        // 证伪。`Node` 是第一个消费者，这里补上——这个测试就是那笔债。
        //
        // 第一版用 group_by=[0]、predicate 读列 1，Project 把行收窄到只剩
        // 1 列，于是"用错下标"必然越界 panic——这实际钉住的是"下标别越界"，
        // 不是 spec 要求的"谓词必须按基表下标求值"；一次把 `Row::get` 换成
        // `i.min(len - 1)` 式钳制的未来重构会让它悄悄变绿，而语义仍然是错的
        // （复审 Finding 1 指出）。
        //
        // 现在改用 arity=3、group_by=[2]、aggs=[Sum(1)]、predicate 读列 0：
        // keep=[2, 1]，narrowed 行仍然是 2 列宽，基表下标 0 与收窄后下标 0
        // 指向两个都存在、但不同的列——用错下标不会 panic，只会算出一个
        // 错误但合法形状的答案。
        let query = ViewQuery {
            group_by: vec![2],
            aggs: vec![Agg {
                func: AggFn::Sum,
                column: Some(1),
            }],
            predicate: Predicate::IntGt {
                column: 0,
                value: 3,
            },
        };
        let plan = lower(&query, "t", 3).expect("合法查询必须能降下来");
        let Plan::Aggregate { input, .. } = plan else {
            panic!("lower 的根算子必须是 Aggregate");
        };
        // Task 3 还没有 Aggregate 节点（Task 4 才加），所以从 Aggregate 的
        // input——也就是 lower() 真实产出的 Filter/Project 子树——建 Node，
        // 而不是手写一棵形状相似的等价树。
        let mut n = Node::build(&input).unwrap();

        // 列 0 = 谓词看的列（会被收窄掉），列 1 = SUM 的列，列 2 = group key。
        let d = ZSet::from_rows([
            (row(vec![int(100), int(5), int(1)]), 1), // 基表列 0: 100 > 3 → 通过
            (row(vec![int(1), int(5), int(200)]), 1), // 基表列 0: 1，不 > 3 → 不通过
        ]);
        let got = n.delta("t", &d);
        assert_eq!(
            got,
            ZSet::from_rows([(row(vec![int(1), int(5)]), 1)]),
            "谓词必须按基表下标（列 0）求值；若按收窄后的下标求值（keep=[2, 1]，\
             收窄后位置 0 实际是基表列 2），会把第二行误判为通过、第一行误判为\
             不通过，得到 {{Row([200, 5]): 1}} 而不是 {{Row([1, 5]): 1}}——\
             是一个错误答案，不是越界 panic"
        );
    }
}
