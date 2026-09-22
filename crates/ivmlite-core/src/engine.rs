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
        // Task 6 会在这里插入 consolidation。现在逐条推进：一条一批。
        for (table, row, w) in std::mem::take(&mut self.pending) {
            let d = ZSet::from_rows([(row, w)]);
            self.view.merge(&tree.delta(&table, &d));
        }
        Ok(())
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
}
