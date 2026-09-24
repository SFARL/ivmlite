use crate::{
    Arrangement, ArrangementId, ArrangementRole, JoinState, Plan, Predicate, Row, Value, ZSet,
};

/// The stateful operator tree. Built from a `Plan`, after which `delta` is called repeatedly.
///
/// It is separate from `Plan` because `Plan` is pure description — comparable,
/// printable, and rebuildable from SQL later — while operators hold state. Spec
/// §5.3's choice to store SQL text rather than a serialized IR depends on this
/// separation.
#[derive(Debug)]
pub enum Node {
    Scan {
        table: String,
    },
    Join {
        left: Box<Node>,
        right: Box<Node>,
        state: JoinState,
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
    /// Build the operator tree from a `Plan`, recursively.
    ///
    /// It used to return `Result<Node, NodeError>`, but every arm of `build`
    /// wrote `Ok(...)` from the start — `NodeError` was never constructed, and
    /// the `.map_err` following it in `engine.rs` was dead code. M1a Phase 2
    /// final review, Finding H: a signature that cannot fail is more honest than
    /// a fake error path. When M1b needs a `build` that genuinely can fail (say,
    /// rebuilding a node from the SQLite side), the compiler will point
    /// mechanically at every call site that must handle the new `Err`, so adding
    /// it back is a local change.
    ///
    /// `arrangements` supplies every arrangement an operator needs, asked for
    /// by `ArrangementId`: the operator's pre-order position in `plan` and the
    /// arrangement's role (M1b Phase 1, Ruling 2). The engine passes
    /// `fresh_mem_arrangement`; a provider that returns non-empty arrangements
    /// rebuilds a tree from persisted state.
    pub fn build(
        plan: &Plan,
        arrangements: &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>,
    ) -> Node {
        let mut next = 0;
        Node::build_at(plan, &mut next, arrangements)
    }

    /// `build`'s recursion: `next` is the pre-order index the next plan node gets.
    fn build_at(
        plan: &Plan,
        next: &mut usize,
        arrangements: &mut dyn FnMut(ArrangementId) -> Box<dyn Arrangement>,
    ) -> Node {
        let node = *next;
        *next += 1;
        match plan {
            Plan::Scan { table, .. } => Node::Scan {
                table: table.clone(),
            },
            Plan::Join {
                left,
                right,
                left_key,
                right_key,
            } => {
                let left = Node::build_at(left, next, arrangements);
                let right = Node::build_at(right, next, arrangements);
                Node::Join {
                    left: Box::new(left),
                    right: Box::new(right),
                    state: JoinState::new(
                        *left_key,
                        *right_key,
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinLeft,
                        }),
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::JoinRight,
                        }),
                    ),
                }
            }
            Plan::Filter { input, predicate } => Node::Filter {
                input: Box::new(Node::build_at(input, next, arrangements)),
                predicate: predicate.clone(),
            },
            Plan::Project { input, columns } => Node::Project {
                input: Box::new(Node::build_at(input, next, arrangements)),
                columns: columns.clone(),
            },
            Plan::Aggregate {
                input,
                group_by,
                aggs,
            } => {
                let input = Node::build_at(input, next, arrangements);
                Node::Aggregate {
                    input: Box::new(input),
                    state: crate::AggState::new(
                        group_by.clone(),
                        aggs.clone(),
                        arrangements(ArrangementId {
                            node,
                            role: ArrangementRole::AggregateGroups,
                        }),
                    ),
                }
            }
        }
    }

    /// Push one batch of deltas for some table through this node, returning the node's output delta.
    ///
    /// Spec §6.1: linear operators satisfy `Δ(f(R)) = f(ΔR)`, so Filter and
    /// Project are stateless and deltas pass straight through.
    pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        match self {
            Node::Scan { table: own } => {
                if own == table {
                    input.clone()
                } else {
                    ZSet::new()
                }
            }
            Node::Join { left, right, state } => {
                // Each child is a `Scan` routing by table name, so for one
                // table's delta at most one of these is non-empty (v0 rejects
                // self-joins in `lower`); `JoinState::absorb` is correct either way.
                let left_delta = left.delta(table, input);
                let right_delta = right.delta(table, input);
                state.absorb(&left_delta, &right_delta)
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
                    // After narrowing a row may coincide with another —
                    // `ZSet::update` adds the weights and removes the entry on
                    // reaching zero, exactly the Z-set semantics needed.
                    let narrowed = Row::new(columns.iter().map(|&c| row.get(c).clone()).collect());
                    out.update(narrowed, w);
                }
                out
            }
            Node::Aggregate {
                input: child,
                state,
            } => {
                // Spec §6.2: aggregation is this engine's only stateful
                // operator. Compute the upstream delta first, then let
                // `AggState` decide what to retract and what to emit.
                let upstream = child.delta(table, input);
                state.absorb(&upstream)
            }
        }
    }
}

/// Spec §6.1's three-valued logic: NULL evaluates to UNKNOWN, and the row is
/// excluded from the result.
///
/// It returns a `bool` rather than a three-valued enum because, for
/// **filtering**, "false" and "unknown" get the same treatment. But **do not
/// conclude from this that `NOT p` is equivalent to `!p`** — v0's predicate
/// whitelist has no `NOT` precisely because each addition means re-arguing
/// three-valued logic.
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(i) => i > value,
            // NULL > 3 is UNKNOWN, so `false` is correct.
            //
            // `false` for Text > Int is a different kind of guarantee: it is
            // not the right answer. SQLite orders types
            // NULL < INTEGER/REAL < TEXT < BLOB, so `'abc' > 3` is `1` (true)
            // in SQLite, not `0`. This arm is sound only because it cannot be
            // reached through `create_view`: `lower` rejects `IntGt` over a
            // non-INTEGER column at the boundary (external review P2-1). Before
            // that it was unreachable only by `enumerate`'s convention, and a
            // hand-built view would have silently disagreed with the oracle —
            // undetectably, since `NaiveRecompute::passes` has the identical
            // collapse. Supporting it would mean implementing storage-class
            // ordering, not returning `true`. See the matching n/a row in
            // docs/mutation-gates.md.
            _ => false,
        },
        Predicate::IsNotNull { column } => !matches!(row.get(*column), Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lower, Agg, AggFn, Predicate, Value, ViewQuery, ZSet};

    /// `t(k TEXT, v INTEGER)` — matches the rows these tests actually push,
    /// whose column 0 is text and column 1 is an integer.
    fn text_then_int() -> crate::Schema {
        crate::Schema {
            table: "t".into(),
            columns: vec![
                crate::Column {
                    name: "k".into(),
                    ty: crate::ColumnType::Text,
                    nullable: true,
                },
                crate::Column {
                    name: "v".into(),
                    ty: crate::ColumnType::Integer,
                    nullable: true,
                },
            ],
        }
    }

    /// `t(c0, c1, c2)`, all INTEGER — matches the all-integer rows the
    /// base-table-index test pushes.
    fn ints3() -> crate::Schema {
        crate::Schema {
            table: "t".into(),
            columns: (0..3)
                .map(|i| crate::Column {
                    name: format!("c{i}"),
                    ty: crate::ColumnType::Integer,
                    nullable: true,
                })
                .collect(),
        }
    }

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    #[test]
    fn scan_only_absorbs_its_own_table() {
        // With one table this looks redundant, but it is the mechanism by which
        // each side of a join absorbs only its own table's deltas — this is
        // what `Node::Join` relies on.
        let mut n = Node::build(
            &Plan::Scan {
                table: "orders".into(),
                columns: vec![0, 1],
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]);
        assert_eq!(
            n.delta("orders", &d),
            d,
            "its own table: passed through unchanged"
        );
        assert_eq!(
            n.delta("customers", &d),
            ZSet::new(),
            "another table: empty"
        );
    }

    #[test]
    fn filter_passes_deltas_through_unchanged_for_matching_rows() {
        // Spec §6.1: linear operators, Δ(f(R)) = f(ΔR) — deltas pass straight
        // through, with no state. Weights must be preserved as they are,
        // including negative ones (retracting a row that satisfies the
        // predicate must pass through too).
        let mut n = Node::build(
            &Plan::Filter {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0],
                }),
                predicate: Predicate::IntGt {
                    column: 0,
                    value: 3,
                },
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([(row(vec![int(5)]), 1), (row(vec![int(9)]), -2)]);
        assert_eq!(n.delta("t", &d), d);
    }

    #[test]
    fn filter_drops_rows_that_fail_the_predicate() {
        let mut n = Node::build(
            &Plan::Filter {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0],
                }),
                predicate: Predicate::IntGt {
                    column: 0,
                    value: 3,
                },
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([(row(vec![int(1)]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn filter_treats_null_as_unknown_not_as_false_negation() {
        // Spec §6.1's three-valued logic: with v in {1, NULL, 5}, `v > 3` matches
        // 1 row and `NOT (v > 3)` also matches only 1 — together 2, not 3. This
        // test pins that the NULL row enters neither side, not that "NULL is
        // equivalent to false".
        let mut n = Node::build(
            &Plan::Filter {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0],
                }),
                predicate: Predicate::IntGt {
                    column: 0,
                    value: 3,
                },
            },
            &mut crate::fresh_mem_arrangement,
        );
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
            "a NULL row must not enter the result"
        );
    }

    #[test]
    fn is_not_null_predicate_filters_null_rows() {
        let mut n = Node::build(
            &Plan::Filter {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0],
                }),
                predicate: Predicate::IsNotNull { column: 0 },
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([(row(vec![Value::Null]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn project_narrows_columns_and_preserves_weights() {
        let mut n = Node::build(
            &Plan::Project {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0, 1, 2],
                }),
                columns: vec![2, 0],
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([(row(vec![int(1), int(2), int(3)]), 4)]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![int(3), int(1)]), 4)]),
            "columns reordered as `columns` gives them, weights preserved"
        );
    }

    #[test]
    fn project_merges_rows_that_become_identical_after_narrowing() {
        // When two rows become the same row after narrowing, their weights must
        // add rather than the later overwriting the earlier — Z-set semantics,
        // and the one non-trivial thing Project does.
        let mut n = Node::build(
            &Plan::Project {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0, 1],
                }),
                columns: vec![0],
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), 3),
        ]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(7)]), 5)]));
    }

    #[test]
    fn project_drops_rows_whose_weights_cancel_after_narrowing() {
        // A row whose weights cancel to 0 after narrowing must disappear (spec §5.1), not stay as a weight-0 entry.
        let mut n = Node::build(
            &Plan::Project {
                input: Box::new(Plan::Scan {
                    table: "t".into(),
                    columns: vec![0, 1],
                }),
                columns: vec![0],
            },
            &mut crate::fresh_mem_arrangement,
        );
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), -2),
        ]);
        assert!(
            n.delta("t", &d).is_empty(),
            "must be empty once the weights cancel"
        );
    }

    #[test]
    fn aggregate_can_be_built_and_runs_through_the_tree() {
        // End to end: push one batch of deltas through a whole Scan → Filter → Project → Aggregate tree.
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
                join: None,
            },
            &crate::Database::single(text_then_int()),
        )
        .unwrap();
        let mut n = Node::build(&plan, &mut crate::fresh_mem_arrangement);
        let d = ZSet::from_rows([
            (row(vec![Value::Text("a".into()), int(9)]), 1),
            (row(vec![Value::Text("a".into()), int(1)]), 1), // stopped by the Filter
        ]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "only the row that passes the predicate is counted"
        );
    }

    #[test]
    fn aggregate_retracts_across_two_batches_through_the_same_node() {
        // **`AggState` must survive between batches.** That is a property
        // independent of "the delta must actually reach `AggState`":
        // `aggregate_can_be_built_and_runs_through_the_tree` pushes only one
        // batch, so the whole retraction protocol was not tested through `Node`
        // at all — measured: changing the `Aggregate` arm to
        // `let mut scratch = state.clone(); scratch.absorb(&upstream)` (with
        // `Clone` temporarily added back to `AggState`) left the suite green.
        //
        // This is not a far-fetched mutation: spec §5.3 stores SQL text rather
        // than a serialized IR, so "build a fresh tree on every refresh" is an
        // entirely plausible refactor, and so is changing `delta` to take
        // `&self`. Either would turn the engine's only stateful operator
        // stateless — emitting only `+1` per batch and never retracting, which
        // is §6.2's "biggest source of bugs".
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
                join: None,
            },
            &crate::Database::single(text_then_int()),
        )
        .unwrap();
        let mut n = Node::build(&plan, &mut crate::fresh_mem_arrangement);

        let first = n.delta(
            "t",
            &ZSet::from_rows([(row(vec![Value::Text("a".into()), int(9)]), 1)]),
        );
        assert_eq!(
            first,
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "first batch: group a appears for the first time, COUNT=1"
        );

        // The second batch goes through the **same** Node. Group a's COUNT goes
        // from 1 to 2, so the row the first batch emitted must be retracted
        // before the new one is emitted — not a single +1 row.
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
            "across batches the COUNT=1 row emitted by the first batch must be retracted; emitting only (a,2) w=+1 means the state did not survive between batches"
        );
    }

    #[test]
    fn filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones() {
        // The debt Task 1 left (see the matching row in docs/mutation-gates.md):
        // the `Filter` predicate `lower()` produces must be evaluated against
        // **base-table** column indices, which holds only while `Filter` sits
        // **below** `Project` (and so still sees the full-width rows). When
        // Task 1 landed, `Plan` had no consumer, so the requirement could not be
        // falsified then. `Node` is the first consumer; this test is that debt.
        //
        // The first version used group_by=[0] with the predicate reading column
        // 1; Project narrowed rows to a single column, so "the wrong index"
        // always panicked out of bounds — which really pinned "do not index out
        // of range", not the spec's "evaluate the predicate against base-table
        // indices". A future refactor clamping `Row::get` like `i.min(len - 1)`
        // would have turned it quietly green while the semantics stayed wrong
        // (re-review Finding 1).
        //
        // It now uses arity=3, group_by=[2], aggs=[Sum(1)], and a predicate on
        // column 0: keep=[2, 1], so the narrowed row is still 2 wide, and base
        // index 0 and narrowed index 0 name two different columns that both
        // exist — the wrong index does not panic, it just computes a wrong but
        // well-formed answer.
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
            join: None,
        };
        let plan =
            lower(&query, &crate::Database::single(ints3())).expect("a legal query must lower");
        let Plan::Aggregate { input, .. } = plan else {
            panic!("lower's root operator must be an Aggregate");
        };
        // Task 3 had no Aggregate node yet (Task 4 added it), so the Node is
        // built from the Aggregate's input — the real Filter/Project subtree
        // lower() produces — rather than a hand-written tree of similar shape.
        let mut n = Node::build(&input, &mut crate::fresh_mem_arrangement);

        // Column 0 = the predicate's column (narrowed away), column 1 = the SUM column, column 2 = the group key.
        let d = ZSet::from_rows([
            (row(vec![int(100), int(5), int(1)]), 1), // base column 0: 100 > 3 → passes
            (row(vec![int(1), int(5), int(200)]), 1), // base column 0: 1, not > 3 → fails
        ]);
        let got = n.delta("t", &d);
        assert_eq!(
            got,
            ZSet::from_rows([(row(vec![int(1), int(5)]), 1)]),
            "the predicate must be evaluated against the base-table index (column 0); \
             evaluated against the narrowed index (keep=[2, 1], where narrowed \
             position 0 is base column 2) it would wrongly pass the second row and \
             fail the first, giving {{Row([200, 5]): 1}} instead of \
             {{Row([1, 5]): 1}} — a wrong answer, not an out-of-bounds panic"
        );
    }

    fn join_plan() -> Plan {
        Plan::Join {
            left: Box::new(Plan::Scan {
                table: "t0".into(),
                columns: vec![0, 1],
            }),
            right: Box::new(Plan::Scan {
                table: "t1".into(),
                columns: vec![0, 1],
            }),
            left_key: 0,
            right_key: 0,
        }
    }

    #[test]
    fn a_join_tree_routes_each_tables_delta_to_its_own_side() {
        let mut n = Node::build(&join_plan(), &mut crate::fresh_mem_arrangement);
        let right_row = Row::new(vec![Value::Text("a".into()), Value::Int(10)]);
        let left_row = Row::new(vec![Value::Text("a".into()), Value::Int(1)]);
        assert!(n.delta("t1", &ZSet::from_rows([(right_row, 1)])).is_empty());
        let out = n.delta("t0", &ZSet::from_rows([(left_row, 1)]));
        assert_eq!(
            out,
            ZSet::from_rows([(
                Row::new(vec![
                    Value::Text("a".into()),
                    Value::Int(1),
                    Value::Text("a".into()),
                    Value::Int(10),
                ]),
                1
            )])
        );
    }

    fn scan(table: &str) -> Plan {
        Plan::Scan {
            table: table.into(),
            columns: vec![0, 1],
        }
    }

    fn count_agg() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    /// Records every id `Node::build` asks its provider for, in order.
    fn ids_asked_for(plan: &Plan) -> Vec<crate::ArrangementId> {
        let mut asked = Vec::new();
        let _ = Node::build(plan, &mut |id| {
            asked.push(id);
            crate::fresh_mem_arrangement(id)
        });
        asked
    }

    fn id(node: usize, role: crate::ArrangementRole) -> crate::ArrangementId {
        crate::ArrangementId { node, role }
    }

    #[test]
    fn build_asks_for_each_join_input_by_node_and_role() {
        use crate::ArrangementRole::{JoinLeft, JoinRight};
        assert_eq!(
            ids_asked_for(&join_plan()),
            vec![id(0, JoinLeft), id(0, JoinRight)]
        );
    }

    #[test]
    fn node_ids_number_the_plan_in_pre_order() {
        // Aggregate(0) → Project(1) → Join(2) → Scan(3), Scan(4): a provider
        // loading persisted state finds an operator's tables by this number,
        // so it must depend only on the plan's shape.
        use crate::ArrangementRole::{AggregateGroups, JoinLeft, JoinRight};
        let plan = Plan::Aggregate {
            input: Box::new(Plan::Project {
                input: Box::new(join_plan()),
                columns: vec![0],
            }),
            group_by: vec![0],
            aggs: vec![count_agg()],
        };
        assert_eq!(
            ids_asked_for(&plan),
            vec![id(2, JoinLeft), id(2, JoinRight), id(0, AggregateGroups)]
        );

        // A Filter is a node too: it shifts the join to position 3.
        let filtered = Plan::Aggregate {
            input: Box::new(Plan::Project {
                input: Box::new(Plan::Filter {
                    input: Box::new(join_plan()),
                    predicate: Predicate::IsNotNull { column: 1 },
                }),
                columns: vec![0],
            }),
            group_by: vec![0],
            aggs: vec![count_agg()],
        };
        assert_eq!(
            ids_asked_for(&filtered),
            vec![id(3, JoinLeft), id(3, JoinRight), id(0, AggregateGroups)]
        );
    }

    #[test]
    fn two_joins_in_one_plan_get_distinct_ids() {
        // `JoinSide` could not tell two joins in one view apart (spec §4.4,
        // M1a Phase 3 amendment); the node index can. Join(0) over Join(1)
        // and Scan t2.
        use crate::ArrangementRole::{JoinLeft, JoinRight};
        let plan = Plan::Join {
            left: Box::new(join_plan()),
            right: Box::new(scan("t2")),
            left_key: 0,
            right_key: 0,
        };
        assert_eq!(
            ids_asked_for(&plan),
            vec![
                id(1, JoinLeft),
                id(1, JoinRight),
                id(0, JoinLeft),
                id(0, JoinRight)
            ]
        );
    }

    #[test]
    fn a_tree_rebuilt_from_its_arrangements_continues_where_the_old_one_left_off() {
        // The whole operator tree of a join view — the join's two sides and
        // the aggregate's groups — rebuilt from nothing but its arrangements'
        // contents, must produce exactly the old tree's next delta, including
        // the retraction of a row only the old tree emitted.
        use crate::test_support::{join_on_k, kv, kv_row, Mirrors};
        let db = crate::Database::new(vec![kv("t0"), kv("t1")]);
        let plan = crate::lower(&join_on_k(), &db).expect("a legal join query must lower");

        let mirrors = Mirrors::default();
        let mut old = Node::build(&plan, &mut |id| mirrors.arrangement(id));
        old.delta("t0", &ZSet::from_rows([(kv_row("a", 1), 1)]));
        old.delta(
            "t1",
            &ZSet::from_rows([(kv_row("a", 10), 1), (kv_row("a", 20), 1)]),
        );

        let mut rebuilt = Node::build(&plan, &mut |id| -> Box<dyn crate::Arrangement> {
            Box::new(mirrors.snapshot(id))
        });
        let next = ZSet::from_rows([(kv_row("a", 5), 1)]);
        let from_rebuilt = rebuilt.delta("t1", &next);
        let from_old = old.delta("t1", &next);

        let out = |count: i64, sum: i64| {
            Row::new(vec![
                Value::Text("a".into()),
                Value::Int(count),
                Value::Int(sum),
            ])
        };
        assert_eq!(
            from_old,
            ZSet::from_rows([(out(2, 30), -1), (out(3, 35), 1)])
        );
        assert_eq!(from_rebuilt, from_old);

        // A `t1` delta probes only the join's left arrangement; a `t0` delta
        // is what reads the right one. (a, 2) meets the three right rows
        // 10, 20 and 5: COUNT goes from 3 to 6 and SUM from 35 to 70.
        let next_left = ZSet::from_rows([(kv_row("a", 2), 1)]);
        let left_from_rebuilt = rebuilt.delta("t0", &next_left);
        let left_from_old = old.delta("t0", &next_left);
        assert_eq!(
            left_from_old,
            ZSet::from_rows([(out(3, 35), -1), (out(6, 70), 1)])
        );
        assert_eq!(left_from_rebuilt, left_from_old);
    }
}
