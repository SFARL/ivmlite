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

/// 被测实现与测试框架之间唯一的接缝。
///
/// M0 提供 NaiveRecompute（平凡正确）与 NoRetractionEngine（故意有 bug）；
/// M1 的真实引擎实现同一个 trait 后即可直接接入全部测试与 benchmark。
pub trait Engine {
    /// `db` 声明用例涉及的全部基表（顺序确定，spec §9.4）；`initial` 按表名
    /// 给出每张基表的初始状态——多表化之后 `create_view` 第一次真的需要
    /// 不止一张表的初始状态（task 5）。
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError>;

    /// 摄入一批**未合并**的变更，指明它们属于哪张表。
    ///
    /// `raw` 刻意是 `&[(Row, i64)]` 而非 `ZSet`：同一行可以在同一批里出现多次，
    /// 引擎必须自己决定要不要先 consolidate。spec §8.2 把 consolidation 列为
    /// M1 的内容而非优化项——如果 harness 替引擎合并好，M1 的 consolidation
    /// 就从这个接缝上结构性不可见。
    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>;

    /// 把已摄入但未应用的变更维护进视图状态。
    ///
    /// 与 `apply` 分离是因为 spec §8.2 规定显式 refresh 是永久 API，而非 v0
    /// 的临时妥协；§9.1 的批次无关性也只有在维护时刻可控时才可测。
    fn refresh(&mut self) -> Result<(), EngineError>;

    /// 取 &mut self：真实引擎读取前可能需要先 drain 待处理的 delta（spec §8.2）。
    fn materialize(&mut self) -> Result<ZSet, EngineError>;
}
