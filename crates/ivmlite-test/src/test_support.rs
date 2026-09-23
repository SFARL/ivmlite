//! Shared helpers for tests only (m4).
//!
//! The concept "wrap one `Schema` as a `Database` / initial state holding only
//! that table" used to be duplicated across the test code of five modules,
//! under three different names with three different signatures:
//! `differential.rs`'s `single_table_case` (building a whole `TestCase`), the
//! byte-identical `single_table_case` in `naive.rs` and `buggy.rs` (building
//! `(Database, BTreeMap<String, ZSet>)`), `ops.rs`'s `single_table_db` +
//! `as_initial`, and `oracle.rs`'s `single_base`. They are gathered in this one
//! file so that when Phase 3 first builds genuine two-table cases, it edits one
//! place rather than five.

use std::collections::BTreeMap;

use ivmlite_core::{Database, Row, Schema, ZSet};

/// Wrap a single `Schema` as a `Database` holding only that table.
pub fn single_table_db(schema: &Schema) -> Database {
    Database::single(schema.clone())
}

/// Build a base-state map, keyed by table name, holding only this table (the
/// `ZSet` version, fed to `Engine::create_view` / `recompute_via_sqlite`).
pub fn single_base(table: &str, base: ZSet) -> BTreeMap<String, ZSet> {
    BTreeMap::from([(table.to_string(), base)])
}

/// Wrap a single-table case as a one-table `Database` with its initial state keyed by table name (the `ZSet` version).
pub fn single_table_bases(schema: &Schema, base: ZSet) -> (Database, BTreeMap<String, ZSet>) {
    (single_table_db(schema), single_base(&schema.table, base))
}

/// Build an initial-rows map, keyed by table name, holding only this table (the
/// `Row` version, fed to interfaces that work in `Vec<Row>`, such as `gen_ops` /
/// `TestCase::initial`).
pub fn as_initial(schema: &Schema, rows: Vec<Row>) -> BTreeMap<String, Vec<Row>> {
    BTreeMap::from([(schema.table.clone(), rows)])
}
