use std::collections::{BTreeMap, BTreeSet};

use crate::{lower, Database, Node, Row, ViewQuery, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// 包住 `Node`，把「一次调用推进了多少行」变成算子树自己记的账，而不是
/// 调用方在调用前另外算出来的数字。
///
/// 最终评审 Finding B：`refresh` 曾经这样写——`rows_processed` 由推进前的
/// `delta.len()` 单独算出来，`tree.delta(table, delta)` 是紧挨着的下一行、
/// 但在语法上与前者毫无关联。把 `refresh` 改成对 `delta` 里每一行各调一次
/// `tree.delta`（而不是整批调一次）之后，两处都能各自独立编译通过、语义
/// 也不变（因为 delta 已经在 `by_table` 那一步合并过），但「consolidation
/// 让算子树少被推进几次」这件事——spec §8.2/§8.5 唯一在意的性能故事——
/// 就从这个计数器身上彻底测不出来了。
///
/// 于是 `pushes`/`rows_fed` 不是「猜」出来的，是 `Node::delta` **真的被
/// 调用时**自己记的——调用方无论把同一批 delta 拆成多少次调用喂进来，
/// 这两个数字都会如实反映。
///
/// 但**不要把 `node` 字段的私有当成结构性保证**：Rust 的字段私有是
/// 模块级的，而 `CountingTree` 与 `IncrementalEngine` 同在本文件里，
/// `refresh` 完全可以写 `tree.node.delta(...)` 绕过计数——终审复审实测
/// 过，那样改能编译。今天绕过去会被抓到（计数停在 0，四个测试变红），
/// 但那是因为现有测试断言的是确切的非零值，不是因为类型挡住了它。
/// 若日后本文件里新增了别的持有 `CountingTree` 的代码，这一点要重新想。
#[derive(Debug)]
struct CountingTree {
    node: Node,
    /// 本次 `refresh` 里 `Node::delta`（顶层入口）被调用的次数。
    /// consolidation 生效时，同一张表在一次 `refresh` 里应当恰好被推进
    /// 一次，不管合并后剩几行。
    pushes: usize,
    /// 本次 `refresh` 里累计喂给 `Node::delta` 的行数——从调用本身的实参
    /// 观察到，不是从推进前的 `ZSet` 独立算出来的。
    rows_fed: usize,
}

impl CountingTree {
    fn new(node: Node) -> Self {
        Self {
            node,
            pushes: 0,
            rows_fed: 0,
        }
    }

    fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        self.pushes += 1;
        self.rows_fed += input.len();
        self.node.delta(table, input)
    }

    fn reset_counts(&mut self) {
        self.pushes = 0;
        self.rows_fed = 0;
    }
}

/// v0 的增量引擎。
///
/// `apply` 只堆 pending，`refresh` 才推进算子树——spec §8.5 要求两者分离，
/// 且 §8.2 规定显式 refresh 是永久 API 而非 v0 的临时妥协。
#[derive(Debug, Default)]
pub struct IncrementalEngine {
    tree: Option<CountingTree>,
    /// `create_view` 声明过的全部表名——`apply` 用它拒绝未声明的表
    /// （最终评审 Finding L）。引擎本身不持有 `Database`，这是唯一的
    /// 记录方式。
    tables: BTreeSet<String>,
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
    ///
    /// 最终评审 Finding B 之后：这个数字来自 `CountingTree::rows_fed`，
    /// 也就是 `Node::delta` 实际被调用时收到的行数总和，而不是 `refresh`
    /// 在调用之前自己另算的 `delta.len()`。
    rows_processed: usize,
    /// 上一次 `refresh` 里 `Node::delta`（顶层入口）被调用的次数——见
    /// `CountingTree` 的文档注释。
    tree_pushes: usize,
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
            // M1b 里表的顺序来自 `__ivm_dep`，不是查询的 FROM 子句——
            // 这里选「第一张」只是 v0 的既有约定（最终评审 Finding I）：
            // 单表用例下 anchor 与「查询所读的表」碰巧重合，但这不是
            // `db.tables()` 的顺序保证，只是碰巧从没被更复杂的用例拆穿过。
            .ok_or_else(|| EngineError("Database 至少要有一张表".into()))?;
        let plan = lower(query, anchor).map_err(|e| EngineError(e.0))?;
        let mut tree = CountingTree::new(Node::build(&plan));

        // bootstrap：把每张表的初始状态当成第一批 delta 推进去。
        // 声明了表却没给初始状态是错误，不是空表——与 oracle 的
        // `missing_base_state_for_a_declared_table_is_an_error` 同一条约定，
        // 这里由 `create_view_errors_when_a_declared_table_has_no_initial_state`
        // 直接钉住（M1a Phase 2 Task 5 复审 Finding 2；差分 harness 的
        // `gen_initial` 总是给每张表填数据，走不到这条路径，所以补一条
        // 不依赖 harness 用例分布的单元测试）。
        //
        // 最终评审 Finding A：bootstrap 循环建在局部变量 `view` 上，只有
        // 整个循环都成功之后才把 `self.view`/`self.tree`/`self.tables`
        // 一起提交。此前 `self.view = ZSet::new()` 在循环之前就直接写进
        // `self` ——循环中途因为某张表缺初始状态而 `?` 提前返回时，
        // `self.view` 已经变成一个只吸收了部分表的半成品，而 `self.tree`
        // 还停在上一次成功的 `create_view` 建的那棵树上——两者从此永久
        // 不一致，且此后任何 `apply`/`refresh` 都不会再报错，只会安静地
        // 算出错误答案。
        let mut view = ZSet::new();
        for schema in db.tables() {
            let base = initial.get(&schema.table).ok_or_else(|| {
                EngineError(format!("表 {} 被声明但没有给出初始状态", schema.table))
            })?;
            view.merge(&tree.delta(&schema.table, base));
        }
        self.view = view;
        self.tables = db.tables().iter().map(|s| s.table.clone()).collect();
        self.tree = Some(tree);
        self.pending.clear();
        Ok(())
    }

    pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        if self.tree.is_none() {
            return Err(EngineError("apply 在 create_view 之前被调用".into()));
        }
        // 最终评审 Finding L：引擎不持有 `Database`，此前对未声明的表名
        // 来者不拒——直接堆进 `pending`，`refresh` 时喂给 `Node::Scan`，
        // 而 `Scan` 只按表名路由（见 `node.rs`），不认识的表名会被它自己
        // 悄悄吃成一个空 delta，`apply` 因此看起来"成功"了，视图却完全
        // 没被这次调用影响到——M1b 里这类表名来自 shadow-table 的接缝，
        // 一旦对不上，这里必须报错而不是产出一个"过期但看着合理"的视图。
        if !self.tables.contains(table) {
            return Err(EngineError(format!(
                "apply 收到未声明的表 {table}；create_view 声明的表是 {:?}",
                self.tables
            )));
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

        tree.reset_counts();
        for (table, delta) in &by_table {
            // 合并后净权重为 0 的行已被 `ZSet::update` 删掉（§5.1），
            // 于是它们根本不会出现在这里；空 delta 的表干脆不推进算子树，
            // 这本身也是 `tree_pushes` 该反映出来的一部分。
            if delta.is_empty() {
                continue;
            }
            self.view.merge(&tree.delta(table, delta));
        }
        self.rows_processed = tree.rows_fed;
        self.tree_pushes = tree.pushes;
        Ok(())
    }

    /// 上一次 `refresh` 实际推进算子树的行数。
    ///
    /// 合并生效时，同一行在一批里出现 5 次只会被推进 1 次；`+1` 与 `-1`
    /// 相消的行会被推进 0 次。这两个数字是 consolidation 唯一的可观测足迹。
    /// 读的是 `CountingTree::rows_fed`——`Node::delta` 真正收到的行数总和。
    pub fn rows_processed_last_refresh(&self) -> usize {
        self.rows_processed
    }

    /// 上一次 `refresh` 里算子树的顶层入口（`Node::delta`）被调用的次数。
    ///
    /// consolidation 生效时，一张表在一次 `refresh` 里无论合并前有多少条
    /// 原始 raw Δ、合并后剩几行，都应当恰好触发一次调用——这是「少推进
    /// 几次」这个性能故事在调用次数这个维度上的直接证据，`rows_processed`
    /// 只从行数维度证明，两者合起来才堵住「按行逐条推进」这类重构
    /// （最终评审 Finding B）。
    pub fn tree_pushes_last_refresh(&self) -> usize {
        self.tree_pushes
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
            e.tree_pushes_last_refresh(),
            1,
            "合并后只剩一行，算子树本来就只该被推进一次"
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
        // 最终评审 Finding B：这三行同属一张表、同一次 refresh，consolidation
        // 的意义正是把它们合并后**整批**一次性推进算子树——不是合并后逐行
        // 各推一次。`rows_processed_last_refresh` 只能证明「进了算子的行数
        // 对不对」，证明不了「进算子的次数对不对」；后者只有靠一个真正在
        // `Node::delta` 调用点上计数的值才能钉住（见 `CountingTree`）。
        assert_eq!(
            e.tree_pushes_last_refresh(),
            1,
            "三行分属同一张表、同一次 refresh，只应向算子树推进一次，不是逐行 push"
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
        assert_eq!(
            e.tree_pushes_last_refresh(),
            2,
            "两张表各自需要一次独立的 push——Scan 按表名路由，一次调用只能带一个表名"
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

    // --- 最终评审 Finding A：create_view 失败不得污染既有状态 ---

    #[test]
    fn a_failed_create_view_does_not_corrupt_existing_state() {
        // 第一次 create_view 成功并推进过一批数据，建立一个正确的基线。
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        let before = e.snapshot();
        assert_eq!(
            before.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1,
            "基线本身必须正确：COUNT(a)=1"
        );

        // 第二次 create_view 声明了 t0/t1 两张表，但 initial 只给了 t0 的
        // 状态——bootstrap 循环处理到 t1 时必然报错。此前的实现会在报错
        // 之前就已经把 self.view 重置成空 ZSet，而 self.tree 还停在第一次
        // create_view 建的那棵树上，两者从此永久不一致（Finding A 的探针：
        // 真实场景下 truth 是 {(a,2),(b,1)}，引擎会报告 {(a,1),(b,1)} 且
        // 永不恢复）。
        let two = Database::new(vec![table("t0"), table("t1")]);
        let err = e
            .create_view(
                &two,
                &count_query(),
                &BTreeMap::from([("t0".to_string(), ZSet::new())]), // t1 缺失
            )
            .expect_err("t1 没有给出初始状态，第二次 create_view 必须报错");
        assert!(err.0.contains("t1"));

        // 失败的第二次 create_view 不得动到既有状态——快照必须与失败前
        // 逐点相等，而不只是"大致差不多"。
        assert_eq!(
            e.snapshot(),
            before,
            "失败的第二次 create_view 不得污染既有视图状态"
        );

        // 且引擎必须仍然是第一次 create_view 建立的那个可用状态：后续
        // apply + refresh 应当继续在旧视图基础上正确前进，而不是在一棵
        // 悬空的树上算出垃圾、或者直接 panic。
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        let after = e.snapshot();
        assert_eq!(
            after.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1,
            "旧状态里 a 的计数不应被失败的 create_view 或之后的操作破坏"
        );
        assert_eq!(
            after.weight_of(&Row::new(vec![Value::Text("b".into()), Value::Int(1)])),
            1,
            "失败之后引擎必须仍能在旧视图上正确前进"
        );
    }

    // --- 最终评审 Finding L：apply 必须拒绝未声明的表 ---

    #[test]
    fn apply_rejects_an_unknown_table() {
        let mut e = engine(); // 只声明了表 "t"
        let err = e
            .apply("nope", &[(row("a", 1), 1)])
            .expect_err("apply 收到 create_view 未声明过的表名必须报错");
        assert!(
            err.0.contains("nope"),
            "错误信息应指名是哪个未声明的表：{}",
            err.0
        );
    }
}
