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
        // `missing_base_state_for_a_declared_table_is_an_error` 同一条约定。
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

    pub fn materialize(&self) -> ZSet {
        self.view.clone()
    }
}
