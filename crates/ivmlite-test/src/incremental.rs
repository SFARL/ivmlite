use std::collections::BTreeMap;

use ivmlite_core::{Database, IncrementalEngine, Row, ZSet};

use crate::{Engine, EngineError, ViewQuery};

/// Plugs the core engine into the differential harness: a local trait for a foreign type, which the orphan rule allows.
///
/// It only carries the error type across: core must not depend on
/// `ivmlite-test` (§4.2, and the reverse dependency would make a cycle), so each
/// side has its own `EngineError`.
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
