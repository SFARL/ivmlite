use std::collections::BTreeMap;

use crate::{lower, Database, Node, Row, ViewQuery, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// v0 的增量引擎。
///
/// `apply` 只堆 pending，`refresh` 才推进算子树——spec §8.5 要求两者分离，
/// 且 §8.2 规定显式 refresh 是永久 API 而非 v0 的临时妥协。
#[derive(Debug, Default)]
pub struct IncrementalEngine {
    tree: Option<Node>,
    /// 视图的当前物化结果。算子发出的 delta 并进这里。
    view: ZSet,
    /// 已摄入但未维护的原始 Δ，**未合并**（§8.5）。
    /// 用 `Vec` 而非按表的 map：本批内的到达顺序要保留到 `refresh`，
    /// 合并与否是 `refresh` 的决定（Task 6）。
    pending: Vec<(String, Row, i64)>,
    /// 上一次 `refresh` 实际推进算子树的行数。
    ///
    /// consolidation 不改变结果、只改变工作量，于是它无法由「输出对不对」
    /// 观察到——spec §8.5 把这种情形称作结构性不可见。这个计数器是它唯一的
    /// 可观测足迹，也是 §11 要求的「写放大有明确数字」的来源。
    rows_processed: usize,
}

impl IncrementalEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        let anchor = db
            .tables()
            .first()
            .ok_or_else(|| EngineError("Database 至少要有一张表".into()))?;
        let plan = lower(query, &anchor.table, anchor.arity()).map_err(|e| EngineError(e.0))?;
        let mut tree = Node::build(&plan).map_err(|e| EngineError(e.0))?;

        // bootstrap：把每张表的初始状态当成第一批 delta 推进去。
        // 声明了表却没给初始状态是错误，不是空表——与 oracle 的
        // `missing_base_state_for_a_declared_table_is_an_error` 同一条约定，
        // 这里由 `create_view_errors_when_a_declared_table_has_no_initial_state`
        // 直接钉住（M1a Phase 2 Task 5 复审 Finding 2；差分 harness 的
        // `gen_initial` 总是给每张表填数据，走不到这条路径，所以补一条
        // 不依赖 harness 用例分布的单元测试）。
        self.view = ZSet::new();
        for schema in db.tables() {
            let base = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!("表 {} 被声明但没有给出初始状态", schema.table))
            })?;
            self.view.merge(&tree.delta(&schema.table, base));
        }
        self.tree = Some(tree);
        self.pending.clear();
        Ok(())
    }

    pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        if self.tree.is_none() {
            return Err(EngineError("apply 在 create_view 之前被调用".into()));
        }
        self.pending
            .extend(raw.iter().map(|(r, w)| (table.to_string(), r.clone(), *w)));
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<(), EngineError> {
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| EngineError("refresh 在 create_view 之前被调用".into()))?;

        // spec §8.2：raw Δ 先按 Z-set 合并同一行的权重，再进算子。
        // 按表分别合并——同样的行值出现在两张表里时跨表相消是错的。
        // `BTreeMap` 而非 `HashMap`：推进顺序进入 delta 流（§9.4）。
        let mut by_table: BTreeMap<String, ZSet> = BTreeMap::new();
        for (table, row, w) in std::mem::take(&mut self.pending) {
            by_table.entry(table).or_default().update(row, w);
        }

        self.rows_processed = 0;
        for (table, delta) in &by_table {
            // 合并后净权重为 0 的行已被 `ZSet::update` 删掉（§5.1），
            // 于是它们根本不会出现在这里。
            self.rows_processed += delta.len();
            if delta.is_empty() {
                continue;
            }
            self.view.merge(&tree.delta(table, delta));
        }
        Ok(())
    }

    /// 上一次 `refresh` 实际推进算子树的行数。
    ///
    /// 合并生效时，同一行在一批里出现 5 次只会被推进 1 次；`+1` 与 `-1`
    /// 相消的行会被推进 0 次。这两个数字是 consolidation 唯一的可观测足迹。
    pub fn rows_processed_last_refresh(&self) -> usize {
        self.rows_processed
    }

    /// 取视图的当前物化结果。
    ///
    /// 命名为 `snapshot` 而不是 `materialize`：固有方法在方法解析里总是
    /// 优先于同名的 trait 方法（`crate::Engine::materialize`），若两者同名，
    /// 任何持有具体 `IncrementalEngine` 类型（而非 `impl Engine` 泛型）的
    /// 调用点——比如 M1b 里 `ivmlite-sqlite` 直接消费这个类型——都会悄悄
    /// 调到这一个而不是 trait 那个，且没有任何编译期信号提醒。M1a Phase 2
    /// Task 5 复审 Finding 1：这个遮蔽当时已经真实发生过一次，逼着
    /// `harness_catches_bugs.rs` 用 UFCS（`Engine::materialize(&mut inc)`）
    /// 绕开；这里把根因（重名）设计掉，而不是在每个调用点绕。
    pub fn snapshot(&self) -> ZSet {
        self.view.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, Value};

    fn table(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![Column {
                name: "a".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        }
    }

    fn count_query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        }
    }

    /// 复审 Finding 2（M1a Phase 2 Task 5）：`create_view` 声明了一张表却没
    /// 在 `initial` 里给出它的状态时必须报错，而不是悄悄把它当空表处理。
    /// harness 生成的用例走不到这条路径——`gen_initial` 总是给 `db.tables()`
    /// 里每一张表都填数据——但 M1b 里手工构造 `initial` 的调用点（比如
    /// SQLite 侧只想为部分表重建视图）会真的踩上它，所以在 `engine.rs`
    /// 自己的单元测试里直接钉住，不依赖差分 harness 的用例分布。
    #[test]
    fn create_view_errors_when_a_declared_table_has_no_initial_state() {
        let db = Database::new(vec![table("t0"), table("t1")]);
        let initial = BTreeMap::from([("t0".to_string(), ZSet::new())]); // t1 缺失
        let mut engine = IncrementalEngine::new();
        let err = engine
            .create_view(&db, &count_query(), &initial)
            .expect_err("t1 没有给出初始状态，必须报错而不是当空表处理");
        assert!(
            err.0.contains("t1"),
            "错误信息必须指名缺失初始状态的那张表：{}",
            err.0
        );
    }

    #[test]
    fn apply_before_create_view_is_an_error() {
        let mut engine = IncrementalEngine::new();
        let row = Row::new(vec![Value::Int(1)]);
        let err = engine
            .apply("t0", &[(row, 1)])
            .expect_err("create_view 之前调用 apply 必须报错");
        assert!(err.0.contains("apply"));
    }

    #[test]
    fn refresh_before_create_view_is_an_error() {
        let mut engine = IncrementalEngine::new();
        let err = engine
            .refresh()
            .expect_err("create_view 之前调用 refresh 必须报错");
        assert!(err.0.contains("refresh"));
    }

    // --- Task 6：delta consolidation ---

    fn db() -> Database {
        Database::new(vec![Schema {
            table: "t".into(),
            columns: vec![
                Column {
                    name: "k".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "v".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        }])
    }

    fn query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::None,
        }
    }

    fn engine() -> IncrementalEngine {
        let mut e = IncrementalEngine::new();
        e.create_view(
            &db(),
            &query(),
            &BTreeMap::from([("t".to_string(), ZSet::new())]),
        )
        .unwrap();
        e
    }

    fn row(k: &str, v: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(v)])
    }

    #[test]
    fn duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators() {
        // spec §8.2/§8.5：同一行在一批里出现 5 次，合并后只推进 1 次。
        // 这是 consolidation 唯一的可观测足迹——它不改变结果，只改变工作量。
        let mut e = engine();
        let raw: Vec<(Row, i64)> = (0..5).map(|_| (row("a", 1), 1)).collect();
        e.apply("t", &raw).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "5 条相同的 raw Δ 必须先合并成 1 条再进算子"
        );
        assert_eq!(
            e.snapshot()
                .weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(5)])),
            1,
            "合并不得改变结果：COUNT 仍是 5"
        );
    }

    #[test]
    fn rows_that_cancel_within_a_batch_never_reach_the_operators() {
        // 同一批里插入又删除同一行，合并后净权重为 0，根本不该进算子。
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("a", 1), -1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 0, "相消的行不得进算子");
        assert!(e.snapshot().is_empty());
    }

    #[test]
    fn distinct_rows_are_not_over_merged() {
        // 反向守护：合并不得把不同的行并成一条。
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("b", 1), 1), (row("a", 2), 1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            3,
            "三行互不相同，一条都不该被并掉"
        );
    }

    #[test]
    fn deltas_for_different_tables_are_consolidated_separately() {
        // 同样的行值出现在两张表里时不得跨表合并——那会让一张表的变更
        // 消掉另一张表的变更。单表时这个形态不存在，join 落地后是常态。
        let two = Database::new(vec![
            Schema {
                table: "t".into(),
                columns: db().tables()[0].columns.clone(),
            },
            Schema {
                table: "u".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let mut e = IncrementalEngine::new();
        e.create_view(
            &two,
            &query(),
            &BTreeMap::from([
                ("t".to_string(), ZSet::new()),
                ("u".to_string(), ZSet::new()),
            ]),
        )
        .unwrap();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.apply("u", &[(row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            2,
            "两张表各自一条，不得跨表相消"
        );
    }

    #[test]
    fn the_counter_resets_between_refreshes() {
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "计数是「上一次 refresh」而非累计"
        );
    }
}
