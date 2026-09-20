use ivmlite_core::ZSet;

use crate::{Schema, ViewQuery};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EngineError {}

/// 被测实现与测试框架之间唯一的接缝。
///
/// M0 提供 NaiveRecompute（平凡正确）与 NoRetractionEngine（故意有 bug）；
/// M1 的真实引擎实现同一个 trait 后即可直接接入全部测试与 benchmark。
pub trait Engine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError>;

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError>;

    /// 取 &mut self：真实引擎读取前可能需要先 drain 待处理的 delta（spec §8.2）。
    fn materialize(&mut self) -> Result<ZSet, EngineError>;
}
