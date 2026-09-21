//! 仅测试用的共享辅助函数（m4）。
//!
//! "把一张 `Schema` 包成只有这一张表的 `Database`/初始状态"这个概念，此前
//! 以三种不同名字、三种不同签名分散重复在五个模块的测试代码里：
//! `differential.rs` 的 `single_table_case`（构造整个 `TestCase`）、
//! `naive.rs` 与 `buggy.rs` 里字节级相同的 `single_table_case`（构造
//! `(Database, BTreeMap<String, ZSet>)`）、`ops.rs` 的 `single_table_db` +
//! `as_initial`、`oracle.rs` 的 `single_base`。集中到这一个文件，好让
//! Phase 3 第一次真正做两表用例编辑时只需要改一处，而不是五处。

use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Schema, ZSet};

/// 把单表 `Schema` 包成一张只有这一张表的 `Database`。
pub fn single_table_db(schema: &Schema) -> Database {
    Database::single(schema.clone())
}

/// 按表名建一份只含这一张表的基表状态映射（`ZSet` 版本，喂给
/// `Engine::create_view` / `recompute_via_sqlite`）。
pub fn single_base(table: &str, base: ZSet) -> BTreeMap<String, ZSet> {
    BTreeMap::from([(table.to_string(), base)])
}

/// 单表用例包成一张表的 `Database`，配上按表名建的初始状态（`ZSet` 版本）。
pub fn single_table_bases(schema: &Schema, base: ZSet) -> (Database, BTreeMap<String, ZSet>) {
    (single_table_db(schema), single_base(&schema.table, base))
}

/// 按表名建一份只含这一张表的初始行状态映射（`Row` 版本，喂给
/// `gen_ops` / `TestCase::initial` 这类以 `Vec<Row>` 为单位的接口）。
pub fn as_initial(schema: &Schema, rows: Vec<Row>) -> BTreeMap<String, Vec<Row>> {
    BTreeMap::from([(schema.table.clone(), rows)])
}
