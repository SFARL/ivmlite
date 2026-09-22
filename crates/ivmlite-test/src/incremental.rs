use std::collections::BTreeMap;

use ivmlite_core::{Database, IncrementalEngine, Row, ZSet};

use crate::{Engine, EngineError, ViewQuery};

/// 把 core 的引擎接进差分框架。本地 trait + 外部类型，孤儿规则允许。
///
/// 只做错误类型的搬运：core 不得依赖 `ivmlite-test`（§4.2，且反向会成环），
/// 所以两边各有一个 `EngineError`。
impl Engine for IncrementalEngine {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        IncrementalEngine::create_view(self, db, query, initial).map_err(|e| EngineError(e.0))
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        IncrementalEngine::apply(self, table, raw).map_err(|e| EngineError(e.0))
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        IncrementalEngine::refresh(self).map_err(|e| EngineError(e.0))
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        Ok(IncrementalEngine::snapshot(self))
    }
}
