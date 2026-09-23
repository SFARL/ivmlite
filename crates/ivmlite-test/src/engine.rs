use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, ZSet};

use crate::ViewQuery;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// The only seam between an implementation under test and the test framework.
///
/// M0 provides NaiveRecompute (trivially correct) and NoRetractionEngine
/// (deliberately buggy); M1's real engine implements the same trait and plugs
/// straight into every test and the benchmark.
pub trait Engine {
    /// `db` declares every base table the case involves (in a deterministic
    /// order, spec §9.4); `initial` gives each base table's initial state, keyed
    /// by table name — since the harness went multi-table, `create_view`
    /// genuinely needs more than one table's initial state (Task 5).
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError>;

    /// Ingest a batch of **unconsolidated** changes, naming the table they belong to.
    ///
    /// `raw` is deliberately `&[(Row, i64)]` rather than a `ZSet`: the same row
    /// can appear several times in one batch, and the engine must decide for
    /// itself whether to consolidate first. Spec §8.2 makes consolidation M1
    /// content rather than an optimization — if the harness merged for the
    /// engine, M1's consolidation would be structurally invisible at this seam.
    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>;

    /// Maintain the ingested but unapplied changes into the view state.
    ///
    /// It is separate from `apply` because spec §8.2 makes explicit refresh a
    /// permanent API, not a temporary v0 compromise, and §9.1's batch
    /// independence is testable only when the moment of maintenance is
    /// controllable.
    fn refresh(&mut self) -> Result<(), EngineError>;

    /// Takes &mut self: a real engine may need to drain pending deltas before reading (spec §8.2).
    fn materialize(&mut self) -> Result<ZSet, EngineError>;
}
