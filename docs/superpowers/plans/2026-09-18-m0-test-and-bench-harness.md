# M0 测试与基准骨架 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在写任何引擎代码之前，建立一套能够证明自己会报错的 IVM 差分测试框架，以及一套能产出基线曲线的 benchmark harness。

**Architecture:** 测试框架通过一个 `Engine` trait 与被测实现解耦。M0 提供两个实现——`NaiveRecompute`（平凡正确，同时充当 benchmark 基线）与 `NoRetractionEngine`（故意植入聚合不撤回旧行的 bug）。正确性判据来自一个**独立的** oracle：把最终状态灌进内存 SQLite 跑原始 SQL。框架必须让前者全绿、后者被抓到并 shrink 成最小用例。

**Tech Stack:** Rust 1.95+、`rusqlite`（bundled SQLite）、`rand`（`StdRng` + seed）、GitHub Actions。M0 不引入 `sqlparser-rs`、不引入 `criterion`。

**Spec:** [`docs/superpowers/specs/2026-09-18-ivmlite-design.md`](../specs/2026-09-18-ivmlite-design.md)

## Global Constraints

- **`ivmlite-core` 不得依赖 `rusqlite` 或 `libsqlite3-sys`**（spec §4.2）。`ivmlite-test` 可以。
- **所有 `unsafe` 与 FFI 只允许出现在 `ivmlite-sqlite`**（spec §4.2）。M0 不创建该 crate，因此 M0 全程零 `unsafe`。
- **v0 的 `Value` 只有 `Null` / `Int` / `Text` 三个变体**。无浮点（spec §9 浮点 SUM 不满足结合律）、无 BLOB。`Real` 与 `Blob` 留到 M4。
- **权重不变量**（spec §5.1）：最终物化状态不得有负权重；权重归零的行必须删除，不得留 `w = 0` 的僵尸行。
- **生成器值域必须窄**（spec §9.2）：每列不同值数量默认 8，NULL 出现概率默认 0.2。
- **所有随机走显式 seed**，失败时打印可重放的 seed（spec §9.4）。
- **`ZSet` 内部用 `BTreeMap` 而非 `HashMap`**，保证迭代顺序确定、测试可复现。
- M0 **不创建** `ivmlite-sql` 与 `ivmlite-sqlite`——它们在 M1 才有内容可放。这是对 spec §11「crate 骨架」的一处收窄，理由是 YAGNI。

---

## File Structure

```
Cargo.toml                              workspace
.github/workflows/ci.yml                fmt + clippy + test
crates/
  ivmlite-core/
    Cargo.toml
    src/lib.rs                          pub use
    src/value.rs                        Value
    src/row.rs                          Row
    src/zset.rs                         ZSet
  ivmlite-test/
    Cargo.toml
    src/lib.rs                          pub use
    src/schema.rs                       Column / ColumnType / Schema
    src/query.rs                        AggFn / Agg / Predicate / ViewQuery / enumerate
    src/data.rs                         Domain / 初始数据生成
    src/ops.rs                          Op / OpGenerator（有偏采样）
    src/engine.rs                       Engine trait / EngineError
    src/naive.rs                        NaiveRecompute
    src/buggy.rs                        NoRetractionEngine
    src/oracle.rs                       recompute_via_sqlite
    src/invariants.rs                   四层断言中的不变量层
    src/differential.rs                 TestCase / Batching / run / Failure
    src/shrink.rs                       保持合法性的缩小器
    tests/harness_catches_bugs.rs       M0 完成判定
  ivmlite-bench/
    Cargo.toml
    src/main.rs                         矩阵驱动 + CSV 输出
    src/baseline.rs                     不维护 / 手写 trigger / 朴素重跑
```

---

## Task 1: Workspace 骨架、`Value`、`Row`、CI

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `.github/workflows/ci.yml`
- Create: `crates/ivmlite-core/Cargo.toml`, `crates/ivmlite-core/src/lib.rs`
- Create: `crates/ivmlite-core/src/value.rs`, `crates/ivmlite-core/src/row.rs`

**Interfaces:**
- Consumes: 无
- Produces: `ivmlite_core::Value`（枚举，变体 `Null` / `Int(i64)` / `Text(String)`）、`ivmlite_core::Row`（newtype `Row(pub Vec<Value>)`，方法 `new(Vec<Value>) -> Row`、`get(usize) -> &Value`、`len() -> usize`）。两者均 derive `Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash`。

- [ ] **Step 1: 写 workspace 与 crate 清单**

`Cargo.toml`：

```toml
[workspace]
resolver = "2"
members = ["crates/ivmlite-core", "crates/ivmlite-test", "crates/ivmlite-bench"]

[workspace.package]
edition = "2021"
rust-version = "1.95"
license = "MIT"
repository = "https://github.com/SFARL/ivmlite"

[workspace.dependencies]
ivmlite-core = { path = "crates/ivmlite-core" }
rusqlite = { version = "0.40", features = ["bundled"] }
rand = "0.10"
```

`.gitignore`：

```
/target
Cargo.lock
```

> Cargo.lock 忽略是因为本仓库当前只产出 library 与内部 bench bin。M1 产出 cdylib 时改为提交 lock。

`crates/ivmlite-core/Cargo.toml`：

```toml
[package]
name = "ivmlite-core"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
```

依赖表为空是**有意为之**，对应 Global Constraints 第一条。

- [ ] **Step 2: 写失败的测试**

`crates/ivmlite-core/src/value.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_and_text_with_same_digits_are_distinct() {
        assert_ne!(Value::Int(1), Value::Text("1".to_string()));
    }

    #[test]
    fn null_orders_before_everything() {
        let mut vs = vec![Value::Text("a".into()), Value::Int(3), Value::Null];
        vs.sort();
        assert_eq!(vs[0], Value::Null);
    }
}
```

`crates/ivmlite-core/src/row.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    #[test]
    fn row_exposes_values_by_index() {
        let r = Row::new(vec![Value::Int(7), Value::Null]);
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(0), &Value::Int(7));
        assert_eq!(r.get(1), &Value::Null);
    }

    #[test]
    fn rows_sort_deterministically() {
        let a = Row::new(vec![Value::Int(1)]);
        let b = Row::new(vec![Value::Int(2)]);
        let mut v = vec![b.clone(), a.clone()];
        v.sort();
        assert_eq!(v, vec![a, b]);
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p ivmlite-core`
Expected: 编译失败，`cannot find type Value` / `cannot find type Row`

- [ ] **Step 4: 写实现**

`crates/ivmlite-core/src/value.rs` 顶部：

```rust
/// v0 的值域。刻意不含 Real 与 Blob：
/// Real 会让增量 SUM 与全量重算无法 bit-for-bit 相等（浮点加法不满足结合律），
/// Blob 在 v0 的 STRICT table 限制下用不到。两者均排在 M4。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    Null,
    Int(i64),
    Text(String),
}
```

`crates/ivmlite-core/src/row.rs` 顶部：

```rust
use crate::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Row(pub Vec<Value>);

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Row(values)
    }

    pub fn get(&self, index: usize) -> &Value {
        &self.0[index]
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

`crates/ivmlite-core/src/lib.rs`：

```rust
mod row;
mod value;

pub use row::Row;
pub use value::Value;
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p ivmlite-core`
Expected: 4 passed

- [ ] **Step 6: 加 CI**

`.github/workflows/ci.yml`：

```yaml
name: ci
on:
  push:
    branches: [master]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 7: 确认本地与 CI 同样的三条命令都过**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: 全部通过。（此时 `ivmlite-test` / `ivmlite-bench` 尚未创建，需先把 workspace `members` 暂时裁到只剩 `ivmlite-core`，在 Task 3 与 Task 12 创建时再加回。）

- [ ] **Step 8: 提交**

```bash
git add Cargo.toml .gitignore .github/workflows/ci.yml crates/ivmlite-core
git commit -m "feat(core): workspace 骨架、Value/Row 数据类型与 CI"
```

---

## Task 2: `ZSet`

**Files:**
- Create: `crates/ivmlite-core/src/zset.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`

**Interfaces:**
- Consumes: `Row`（Task 1）
- Produces: `ivmlite_core::ZSet`，方法 `new() -> ZSet`、`from_rows(impl IntoIterator<Item = (Row, i64)>) -> ZSet`、`update(&mut self, Row, i64)`、`merge(&mut self, &ZSet)`、`weight_of(&self, &Row) -> i64`、`iter(&self) -> impl Iterator<Item = (&Row, &i64)>`、`len(&self) -> usize`、`is_empty(&self) -> bool`。derive `Debug, Clone, Default, PartialEq, Eq`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/zset.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Int(n)])
    }

    #[test]
    fn weights_accumulate() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), 2);
        assert_eq!(z.weight_of(&row(1)), 3);
    }

    #[test]
    fn zero_weight_rows_are_removed_not_kept() {
        let mut z = ZSet::new();
        z.update(row(1), 1);
        z.update(row(1), -1);
        assert_eq!(z.weight_of(&row(1)), 0);
        assert_eq!(z.len(), 0, "权重归零的行必须删除，不得留 w=0 的僵尸行");
        assert!(z.is_empty());
    }

    #[test]
    fn update_with_zero_weight_is_a_noop() {
        let mut z = ZSet::new();
        z.update(row(1), 0);
        assert_eq!(z.len(), 0);
    }

    #[test]
    fn negative_weights_are_representable() {
        let mut z = ZSet::new();
        z.update(row(1), -2);
        assert_eq!(z.weight_of(&row(1)), -2);
    }

    #[test]
    fn merge_is_pointwise_addition() {
        let mut a = ZSet::from_rows([(row(1), 1), (row(2), 5)]);
        let b = ZSet::from_rows([(row(1), -1), (row(3), 2)]);
        a.merge(&b);
        assert_eq!(a.weight_of(&row(1)), 0);
        assert_eq!(a.weight_of(&row(2)), 5);
        assert_eq!(a.weight_of(&row(3)), 2);
        assert_eq!(a.len(), 2, "row(1) 归零后应被移除");
    }

    #[test]
    fn iteration_order_is_deterministic() {
        let a = ZSet::from_rows([(row(3), 1), (row(1), 1), (row(2), 1)]);
        let seen: Vec<i64> = a
            .iter()
            .map(|(r, _)| match r.get(0) {
                Value::Int(n) => *n,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(seen, vec![1, 2, 3], "BTreeMap 保证顺序，HashMap 不保证");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-core zset`
Expected: 编译失败，`cannot find type ZSet`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/zset.rs` 顶部：

```rust
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use crate::Row;

/// 带权重的多重集。权重为 i64：INSERT = +1，DELETE = -1。
///
/// 用 BTreeMap 而非 HashMap，是为了让迭代顺序确定——差分测试的失败用例
/// 必须能凭 seed 精确重放。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZSet {
    inner: BTreeMap<Row, i64>,
}

impl ZSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows(rows: impl IntoIterator<Item = (Row, i64)>) -> Self {
        let mut z = Self::new();
        for (row, weight) in rows {
            z.update(row, weight);
        }
        z
    }

    /// 把 `weight` 加到 `row` 现有的权重上。归零的行会被移除。
    pub fn update(&mut self, row: Row, weight: i64) {
        if weight == 0 {
            return;
        }
        match self.inner.entry(row) {
            Entry::Occupied(mut slot) => {
                let combined = *slot.get() + weight;
                if combined == 0 {
                    slot.remove();
                } else {
                    *slot.get_mut() = combined;
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(weight);
            }
        }
    }

    pub fn merge(&mut self, other: &ZSet) {
        for (row, weight) in other.iter() {
            self.update(row.clone(), *weight);
        }
    }

    pub fn weight_of(&self, row: &Row) -> i64 {
        self.inner.get(row).copied().unwrap_or(0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Row, &i64)> {
        self.inner.iter()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
```

`crates/ivmlite-core/src/lib.rs` 追加：

```rust
mod zset;
pub use zset::ZSet;
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-core`
Expected: 10 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-core/src/zset.rs crates/ivmlite-core/src/lib.rs
git commit -m "feat(core): ZSet 与权重归零即删除的语义"
```

---

## Task 3: `Schema` 与 `ivmlite-test` crate

**Files:**
- Create: `crates/ivmlite-test/Cargo.toml`, `crates/ivmlite-test/src/lib.rs`, `crates/ivmlite-test/src/schema.rs`
- Modify: `Cargo.toml`（把 `ivmlite-test` 加回 members）

**Interfaces:**
- Consumes: `ivmlite_core::{Row, Value, ZSet}`
- Produces: `ColumnType`（`Integer` / `Text`）、`Column { name: String, ty: ColumnType, nullable: bool }`、`Schema { table: String, columns: Vec<Column> }`，方法 `Schema::create_table_sql(&self) -> String`、`Schema::arity(&self) -> usize`、`Schema::column_names(&self) -> Vec<&str>`。

- [ ] **Step 1: 建 crate 清单**

`crates/ivmlite-test/Cargo.toml`：

```toml
[package]
name = "ivmlite-test"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
ivmlite-core.workspace = true
rusqlite.workspace = true
rand.workspace = true
```

并把 `ivmlite-test` 加回根 `Cargo.toml` 的 `members`。

- [ ] **Step 2: 写失败的测试**

`crates/ivmlite-test/src/schema.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn create_table_sql_is_strict() {
        let sql = orders().create_table_sql();
        assert!(sql.contains("STRICT"), "spec §7.1 要求 STRICT table：{sql}");
        assert!(sql.contains("\"region\" TEXT"));
        assert!(sql.contains("\"amount\" INTEGER NOT NULL"));
    }

    #[test]
    fn arity_counts_columns() {
        assert_eq!(orders().arity(), 2);
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p ivmlite-test`
Expected: 编译失败，`cannot find type Schema`

- [ ] **Step 4: 写实现**

`crates/ivmlite-test/src/schema.rs` 顶部：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    Text,
}

impl ColumnType {
    fn sql(self) -> &'static str {
        match self {
            ColumnType::Integer => "INTEGER",
            ColumnType::Text => "TEXT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
    pub nullable: bool,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub table: String,
    pub columns: Vec<Column>,
}

impl Schema {
    /// 生成 STRICT 建表语句。STRICT 是 v0 的硬性要求：它把列类型钉死，
    /// 从而消灭 SQLite 的 type affinity 导致 group key 分裂的整类问题（spec §7.1）。
    pub fn create_table_sql(&self) -> String {
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                let null = if c.nullable { "" } else { " NOT NULL" };
                format!("\"{}\" {}{}", c.name, c.ty.sql(), null)
            })
            .collect();
        format!(
            "CREATE TABLE \"{}\" ({}) STRICT",
            self.table,
            cols.join(", ")
        )
    }

    pub fn arity(&self) -> usize {
        self.columns.len()
    }

    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}
```

`crates/ivmlite-test/src/lib.rs`：

```rust
mod schema;

pub use schema::{Column, ColumnType, Schema};
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 2 passed

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml crates/ivmlite-test
git commit -m "feat(test): ivmlite-test crate 与 STRICT schema"
```

---

## Task 4: `ViewQuery` 与 query 空间穷举

**Files:**
- Create: `crates/ivmlite-test/src/query.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ColumnType`（Task 3）
- Produces: `AggFn`（`Count` / `Sum`）、`Agg { func: AggFn, column: Option<usize> }`、`Predicate`（`None` / `IntGt { column, value }` / `IsNotNull { column }`）、`ViewQuery { group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate }`，方法 `ViewQuery::to_sql(&self, &Schema) -> String`、`ViewQuery::output_arity(&self) -> usize`；自由函数 `enumerate(&Schema) -> Vec<ViewQuery>`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/query.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn to_sql_renders_group_by_and_aggs() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        assert_eq!(
            q.to_sql(&orders()),
            "SELECT \"region\", SUM(\"amount\"), COUNT(*) FROM \"orders\" GROUP BY \"region\""
        );
    }

    #[test]
    fn to_sql_renders_predicate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::IntGt { column: 1, value: 3 },
        };
        assert!(q.to_sql(&orders()).contains("WHERE \"amount\" > 3"));
    }

    #[test]
    fn output_arity_is_group_by_plus_aggs() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        assert_eq!(q.output_arity(), 3);
    }

    #[test]
    fn enumerate_covers_the_v0_space_and_is_nonempty() {
        let qs = enumerate(&orders());
        assert!(!qs.is_empty());
        assert!(qs.iter().all(|q| !q.group_by.is_empty()), "v0 要求至少一个 group-by 键");
        assert!(qs.iter().all(|q| !q.aggs.is_empty()), "无聚合的视图不在 v0 范围");
        assert!(qs.iter().any(|q| matches!(q.predicate, Predicate::None)));
        assert!(qs.iter().any(|q| matches!(q.predicate, Predicate::IntGt { .. })));
    }

    #[test]
    fn enumerate_only_sums_integer_columns() {
        let schema = orders();
        for q in enumerate(&schema) {
            for agg in &q.aggs {
                if agg.func == AggFn::Sum {
                    let idx = agg.column.expect("SUM 必须有列");
                    assert_eq!(
                        schema.columns[idx].ty,
                        ColumnType::Integer,
                        "v0 无浮点，SUM 只能作用于 INTEGER 列"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test query`
Expected: 编译失败，`cannot find type ViewQuery`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/query.rs` 顶部：

```rust
use crate::{ColumnType, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Count,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agg {
    pub func: AggFn,
    /// COUNT(*) 为 None；SUM 必须为 Some，且指向 INTEGER 列。
    pub column: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    None,
    IntGt { column: usize, value: i64 },
    IsNotNull { column: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewQuery {
    /// v0 只允许裸列作为 group-by 键，不允许表达式（spec §7.1）。
    pub group_by: Vec<usize>,
    pub aggs: Vec<Agg>,
    pub predicate: Predicate,
}

impl ViewQuery {
    pub fn output_arity(&self) -> usize {
        self.group_by.len() + self.aggs.len()
    }

    pub fn to_sql(&self, schema: &Schema) -> String {
        let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

        let mut select: Vec<String> = self.group_by.iter().map(|i| name(*i)).collect();
        for agg in &self.aggs {
            select.push(match (agg.func, agg.column) {
                (AggFn::Count, _) => "COUNT(*)".to_string(),
                (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
                (AggFn::Sum, None) => unreachable!("SUM 必须指定列"),
            });
        }

        let where_clause = match &self.predicate {
            Predicate::None => String::new(),
            Predicate::IntGt { column, value } => {
                format!(" WHERE {} > {}", name(*column), value)
            }
            Predicate::IsNotNull { column } => {
                format!(" WHERE {} IS NOT NULL", name(*column))
            }
        };

        let group = self
            .group_by
            .iter()
            .map(|i| name(*i))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "SELECT {} FROM \"{}\"{} GROUP BY {}",
            select.join(", "),
            schema.table,
            where_clause,
            group
        )
    }
}

/// 穷举 v0 的 query 空间。
///
/// spec §9.2：v0 的组合数有限，穷举优于随机——可复现且覆盖完全。
/// 随机性留给更新序列。
pub fn enumerate(schema: &Schema) -> Vec<ViewQuery> {
    let int_cols: Vec<usize> = (0..schema.arity())
        .filter(|i| schema.columns[*i].ty == ColumnType::Integer)
        .collect();

    let mut group_by_choices: Vec<Vec<usize>> = Vec::new();
    for i in 0..schema.arity() {
        group_by_choices.push(vec![i]);
        for j in (i + 1)..schema.arity() {
            group_by_choices.push(vec![i, j]);
        }
    }

    let mut agg_choices: Vec<Vec<Agg>> = vec![vec![Agg { func: AggFn::Count, column: None }]];
    for i in &int_cols {
        agg_choices.push(vec![Agg { func: AggFn::Sum, column: Some(*i) }]);
        agg_choices.push(vec![
            Agg { func: AggFn::Sum, column: Some(*i) },
            Agg { func: AggFn::Count, column: None },
        ]);
    }

    let mut predicates = vec![Predicate::None];
    for i in &int_cols {
        predicates.push(Predicate::IntGt { column: *i, value: 4 });
    }
    for i in 0..schema.arity() {
        if schema.columns[i].nullable {
            predicates.push(Predicate::IsNotNull { column: i });
        }
    }

    let mut out = Vec::new();
    for group_by in &group_by_choices {
        for aggs in &agg_choices {
            for predicate in &predicates {
                out.push(ViewQuery {
                    group_by: group_by.clone(),
                    aggs: aggs.clone(),
                    predicate: predicate.clone(),
                });
            }
        }
    }
    out
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod query;
pub use query::{enumerate, Agg, AggFn, Predicate, ViewQuery};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 7 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/query.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): ViewQuery 与 v0 query 空间穷举"
```

---

## Task 5: 数据生成器（窄值域、高 NULL 率）

**Files:**
- Create: `crates/ivmlite-test/src/data.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ColumnType`（Task 3）、`ivmlite_core::{Row, Value}`
- Produces: `Domain { distinct: usize, null_rate: f64 }`（`Default` 为 `distinct: 8, null_rate: 0.2`）、`gen_row(&mut StdRng, &Schema, &Domain) -> Row`、`gen_rows(&mut StdRng, &Schema, &Domain, usize) -> Vec<Row>`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/data.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Schema};
    use rand::SeedableRng;
    use std::collections::HashSet;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn domain_is_narrow_by_default() {
        let d = Domain::default();
        assert_eq!(d.distinct, 8, "窄值域是抓 retraction bug 的前提（spec §9.2）");
    }

    #[test]
    fn generated_values_stay_within_the_domain() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        let schema = orders();
        let domain = Domain::default();
        let rows = gen_rows(&mut rng, &schema, &domain, 500);

        let distinct_regions: HashSet<&Value> = rows.iter().map(|r| r.get(0)).collect();
        assert!(
            distinct_regions.len() <= domain.distinct + 1,
            "不同值数量必须受 domain 限制（+1 容纳 NULL），实得 {}",
            distinct_regions.len()
        );
    }

    #[test]
    fn nullable_columns_actually_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(2);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        let nulls = rows.iter().filter(|r| r.get(0) == &Value::Null).count();
        assert!(nulls > 0, "NULL 在 GROUP BY 中自成一组，是经典 bug 点，必须高频出现");
    }

    #[test]
    fn non_nullable_columns_never_produce_nulls() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(3);
        let rows = gen_rows(&mut rng, &orders(), &Domain::default(), 500);
        assert!(rows.iter().all(|r| r.get(1) != &Value::Null));
    }

    #[test]
    fn same_seed_yields_same_rows() {
        let schema = orders();
        let a = gen_rows(&mut rand::rngs::StdRng::seed_from_u64(7), &schema, &Domain::default(), 20);
        let b = gen_rows(&mut rand::rngs::StdRng::seed_from_u64(7), &schema, &Domain::default(), 20);
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test data`
Expected: 编译失败，`cannot find type Domain`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/data.rs` 顶部：

```rust
use ivmlite_core::{Row, Value};
use rand::rngs::StdRng;
use rand::Rng;

use crate::{ColumnType, Schema};

/// 生成器的值域配置。
///
/// `distinct` 刻意很小：若某列有上百万个不同值，每个 group 只有一行，
/// 就永远测不到"同一个 group 反复增删"——而那正是 retraction 与僵尸行
/// bug 的产地（spec §9.2）。
#[derive(Debug, Clone)]
pub struct Domain {
    pub distinct: usize,
    pub null_rate: f64,
}

impl Default for Domain {
    fn default() -> Self {
        Domain { distinct: 8, null_rate: 0.2 }
    }
}

pub fn gen_row(rng: &mut StdRng, schema: &Schema, domain: &Domain) -> Row {
    let values = schema
        .columns
        .iter()
        .map(|col| {
            if col.nullable && rng.random_bool(domain.null_rate) {
                return Value::Null;
            }
            let n = rng.random_range(0..domain.distinct) as i64;
            match col.ty {
                ColumnType::Integer => Value::Int(n),
                ColumnType::Text => Value::Text(format!("v{n}")),
            }
        })
        .collect();
    Row::new(values)
}

pub fn gen_rows(rng: &mut StdRng, schema: &Schema, domain: &Domain, count: usize) -> Vec<Row> {
    (0..count).map(|_| gen_row(rng, schema, domain)).collect()
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod data;
pub use data::{gen_row, gen_rows, Domain};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 12 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/data.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): 窄值域、高 NULL 率的数据生成器"
```

---

## Task 6: 有偏更新序列生成器

**Files:**
- Create: `crates/ivmlite-test/src/ops.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `Domain`, `gen_row`（Task 3、5）、`ivmlite_core::Row`
- Produces: `Op`（`Insert(Row)` / `Delete(Row)` / `Update { old: Row, new: Row }`）、`Op::to_delta(&self) -> Vec<(Row, i64)>`、`gen_ops(&mut StdRng, &Schema, &Domain, &[Row], usize) -> Vec<Op>`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/ops.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Domain, Schema};
    use ivmlite_core::Value;
    use rand::SeedableRng;

    fn orders() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn row(n: i64) -> Row {
        Row::new(vec![Value::Text(format!("v{n}")), Value::Int(n)])
    }

    #[test]
    fn update_becomes_retract_plus_insert() {
        let op = Op::Update { old: row(1), new: row(2) };
        assert_eq!(op.to_delta(), vec![(row(1), -1), (row(2), 1)]);
    }

    #[test]
    fn insert_and_delete_map_to_plus_and_minus_one() {
        assert_eq!(Op::Insert(row(1)).to_delta(), vec![(row(1), 1)]);
        assert_eq!(Op::Delete(row(1)).to_delta(), vec![(row(1), -1)]);
    }

    #[test]
    fn deletes_target_rows_that_actually_exist() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let schema = orders();
        let domain = Domain::default();
        let initial = crate::gen_rows(&mut rng, &schema, &domain, 40);
        let ops = gen_ops(&mut rng, &schema, &domain, &initial, 300);

        // 重放序列，验证每个 DELETE / UPDATE 命中的行当时确实存在。
        let mut live: Vec<Row> = initial.clone();
        let mut hits = 0usize;
        for op in &ops {
            match op {
                Op::Insert(r) => live.push(r.clone()),
                Op::Delete(r) => {
                    let pos = live.iter().position(|x| x == r);
                    assert!(pos.is_some(), "DELETE 必须命中存在的行");
                    live.remove(pos.unwrap());
                    hits += 1;
                }
                Op::Update { old, new } => {
                    let pos = live.iter().position(|x| x == old);
                    assert!(pos.is_some(), "UPDATE 必须命中存在的行");
                    live.remove(pos.unwrap());
                    live.push(new.clone());
                    hits += 1;
                }
            }
        }
        assert!(
            hits > ops.len() / 10,
            "有偏采样必须产生足量的删改，否则测不到 retraction；实得 {hits}/{}",
            ops.len()
        );
    }

    #[test]
    fn sequence_is_reproducible_from_seed() {
        let schema = orders();
        let domain = Domain::default();
        let make = || {
            let mut rng = rand::rngs::StdRng::seed_from_u64(99);
            let initial = crate::gen_rows(&mut rng, &schema, &domain, 10);
            gen_ops(&mut rng, &schema, &domain, &initial, 50)
        };
        assert_eq!(make(), make());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test ops`
Expected: 编译失败，`cannot find type Op`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/ops.rs` 顶部：

```rust
use ivmlite_core::Row;
use rand::rngs::StdRng;
use rand::Rng;

use crate::{gen_row, Domain, Schema};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Insert(Row),
    Delete(Row),
    Update { old: Row, new: Row },
}

impl Op {
    /// UPDATE 拆成 retract + insert——写进 delta 的内容本身已经是 Z-set（spec §8.1）。
    pub fn to_delta(&self) -> Vec<(Row, i64)> {
        match self {
            Op::Insert(r) => vec![(r.clone(), 1)],
            Op::Delete(r) => vec![(r.clone(), -1)],
            Op::Update { old, new } => vec![(old.clone(), -1), (new.clone(), 1)],
        }
    }
}

/// 生成有偏的更新序列。
///
/// spec §9.2：纯随机生成器在 IVM 测试里几乎抓不到 bug——随机 DELETE 很少
/// 命中真实存在的行。这里维护一份 live 行集合，DELETE / UPDATE 一律从中采样，
/// 于是"删掉刚插入的行"和"把一个 group 删空再填回来"会自然高频发生。
pub fn gen_ops(
    rng: &mut StdRng,
    schema: &Schema,
    domain: &Domain,
    initial: &[Row],
    count: usize,
) -> Vec<Op> {
    let mut live: Vec<Row> = initial.to_vec();
    let mut ops = Vec::with_capacity(count);

    for _ in 0..count {
        // live 为空时只能插入。
        let choice = if live.is_empty() { 0 } else { rng.random_range(0..10) };
        match choice {
            0..=3 => {
                let r = gen_row(rng, schema, domain);
                live.push(r.clone());
                ops.push(Op::Insert(r));
            }
            4..=6 => {
                let idx = rng.random_range(0..live.len());
                let r = live.swap_remove(idx);
                ops.push(Op::Delete(r));
            }
            _ => {
                let idx = rng.random_range(0..live.len());
                let old = live.swap_remove(idx);
                let new = gen_row(rng, schema, domain);
                live.push(new.clone());
                ops.push(Op::Update { old, new });
            }
        }
    }
    ops
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod ops;
pub use ops::{gen_ops, Op};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 16 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/ops.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): 有偏采样的更新序列生成器"
```

---

## Task 7: `Engine` trait 与 `NaiveRecompute`

**Files:**
- Create: `crates/ivmlite-test/src/engine.rs`, `crates/ivmlite-test/src/naive.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ViewQuery`, `AggFn`, `Predicate`（Task 3、4）、`ivmlite_core::{Row, Value, ZSet}`
- Produces: `EngineError`（`struct EngineError(pub String)`）、`trait Engine { fn create_view(&mut self, &Schema, &ViewQuery, &ZSet) -> Result<(), EngineError>; fn apply(&mut self, &ZSet) -> Result<(), EngineError>; fn materialize(&mut self) -> Result<ZSet, EngineError>; }`、`NaiveRecompute::new() -> NaiveRecompute`。

> `materialize` 取 `&mut self`：真实引擎在读取时可能需要先 drain 待处理的 delta（spec §8.2）。这个签名从 M0 就要定对，否则 M1 接入时要改所有调用点。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/naive.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
    use ivmlite_core::{Row, Value};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn sum_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        }
    }

    fn row(region: &str, amount: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(amount)])
    }

    fn out(region: Value, sum: i64, count: i64) -> Row {
        Row::new(vec![region, Value::Int(sum), Value::Int(count)])
    }

    #[test]
    fn aggregates_initial_state() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1), (row("b", 3), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 15, 2)), 1);
        assert_eq!(got.weight_of(&out(Value::Text("b".into()), 3, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn applying_a_delete_updates_the_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply(&ZSet::from_rows([(row("a", 5), -1)])).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 10, 1)), 1);
        assert_eq!(got.len(), 1, "旧的 (a,15,2) 必须消失");
    }

    #[test]
    fn emptying_a_group_removes_it_entirely() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        e.apply(&ZSet::from_rows([(row("a", 10), -1)])).unwrap();

        assert!(e.materialize().unwrap().is_empty(), "空 group 不得留下僵尸行");
    }

    #[test]
    fn null_forms_its_own_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Null, Value::Int(4)]), 1),
            (row("a", 1), 1),
        ]);
        e.create_view(&schema(), &sum_by_region(), &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Null, 4, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn predicate_filters_before_aggregating() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::IntGt { column: 1, value: 4 },
        };
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 1), 1)]);
        e.create_view(&schema(), &q, &base).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test naive`
Expected: 编译失败，`cannot find type NaiveRecompute`

- [ ] **Step 3: 写 `Engine` trait**

`crates/ivmlite-test/src/engine.rs`：

```rust
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
```

- [ ] **Step 4: 写 `NaiveRecompute`**

`crates/ivmlite-test/src/naive.rs` 顶部：

```rust
use std::collections::BTreeMap;

use ivmlite_core::{Row, Value, ZSet};

use crate::{AggFn, Engine, EngineError, Predicate, Schema, ViewQuery};

/// 平凡正确的参照实现：保存全量基表，每次 materialize 重算一遍。
///
/// 两个用途：验证测试框架不会误报；充当 benchmark 的"朴素重跑"基线（spec §10.2）。
#[derive(Debug, Default)]
pub struct NaiveRecompute {
    query: Option<ViewQuery>,
    base: ZSet,
}

impl NaiveRecompute {
    pub fn new() -> Self {
        Self::default()
    }
}

fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(n) => n > value,
            _ => false, // NULL 与非整数一律不通过，与 SQL 的三值逻辑一致
        },
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}

impl Engine for NaiveRecompute {
    fn create_view(
        &mut self,
        _schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.query = Some(query.clone());
        self.base = initial.clone();
        Ok(())
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.base.merge(delta);
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let query = self
            .query
            .as_ref()
            .ok_or_else(|| EngineError("materialize 前未 create_view".into()))?;

        // group key -> (每个 agg 的累加器, 组内行数)
        let mut groups: BTreeMap<Vec<Value>, Vec<i64>> = BTreeMap::new();

        for (row, weight) in self.base.iter() {
            if *weight <= 0 {
                continue;
            }
            if !passes(&query.predicate, row) {
                continue;
            }
            let key: Vec<Value> = query.group_by.iter().map(|i| row.get(*i).clone()).collect();
            let acc = groups.entry(key).or_insert_with(|| vec![0; query.aggs.len()]);
            for (slot, agg) in acc.iter_mut().zip(&query.aggs) {
                match (agg.func, agg.column) {
                    (AggFn::Count, _) => *slot += weight,
                    (AggFn::Sum, Some(col)) => {
                        if let Value::Int(n) = row.get(col) {
                            *slot += n * weight;
                        }
                    }
                    (AggFn::Sum, None) => {
                        return Err(EngineError("SUM 缺少列".into()));
                    }
                }
            }
        }

        let mut out = ZSet::new();
        for (key, acc) in groups {
            let mut values = key;
            values.extend(acc.into_iter().map(Value::Int));
            out.update(Row::new(values), 1);
        }
        Ok(out)
    }
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod engine;
mod naive;
pub use engine::{Engine, EngineError};
pub use naive::NaiveRecompute;
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 21 passed

- [ ] **Step 6: 提交**

```bash
git add crates/ivmlite-test/src/engine.rs crates/ivmlite-test/src/naive.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): Engine trait 与 NaiveRecompute 参照实现"
```

---

## Task 8: SQLite oracle

**Files:**
- Create: `crates/ivmlite-test/src/oracle.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `Schema`, `ViewQuery`（Task 3、4）、`ivmlite_core::{Row, Value, ZSet}`
- Produces: `recompute_via_sqlite(&Schema, &ViewQuery, &ZSet) -> Result<ZSet, EngineError>`

> **为什么 oracle 必须独立于 `NaiveRecompute`**：拿 `NaiveRecompute` 当 oracle 去测 `NaiveRecompute` 是循环论证。真正的判据来自 SQLite 自己执行原始 SQL——一个完全独立的实现。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/oracle.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    fn row(region: Value, amount: i64) -> Row {
        Row::new(vec![region, Value::Int(amount)])
    }

    #[test]
    fn matches_hand_computed_aggregate() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg { func: AggFn::Sum, column: Some(1) },
                Agg { func: AggFn::Count, column: None },
            ],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([
            (row(Value::Text("a".into()), 10), 1),
            (row(Value::Text("a".into()), 5), 1),
            (row(Value::Text("b".into()), 3), 1),
        ]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Int(15),
                Value::Int(2)
            ])),
            1
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn expands_rows_by_weight() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), 3)]);

        let got = recompute_via_sqlite(&schema(), &q, &base).unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(3)])),
            1,
            "权重 3 应展开为 3 行，COUNT(*) 得 3"
        );
    }

    #[test]
    fn rejects_negative_weights_in_base_state() {
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        };
        let base = ZSet::from_rows([(row(Value::Text("a".into()), 1), -1)]);
        assert!(
            recompute_via_sqlite(&schema(), &q, &base).is_err(),
            "基表状态出现负权重说明上游已经错了，oracle 必须拒绝而非静默"
        );
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test oracle`
Expected: 编译失败，`cannot find function recompute_via_sqlite`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/oracle.rs` 顶部：

```rust
use ivmlite_core::{Row, Value, ZSet};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, ToSql};

use crate::{EngineError, Schema, ViewQuery};

struct Bound<'a>(&'a Value);

impl ToSql for Bound<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self.0 {
            Value::Null => ToSqlOutput::Owned(rusqlite::types::Value::Null),
            Value::Int(n) => ToSqlOutput::Owned(rusqlite::types::Value::Integer(*n)),
            Value::Text(s) => ToSqlOutput::Owned(rusqlite::types::Value::Text(s.clone())),
        })
    }
}

fn from_sqlite(v: ValueRef<'_>) -> Result<Value, EngineError> {
    match v {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(n) => Ok(Value::Int(n)),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(|s| Value::Text(s.to_string()))
            .map_err(|e| EngineError(format!("非 UTF-8 文本: {e}"))),
        ValueRef::Real(_) => Err(EngineError("v0 不支持 REAL".into())),
        ValueRef::Blob(_) => Err(EngineError("v0 不支持 BLOB".into())),
    }
}

/// 权威判据：把基表状态灌进内存 SQLite，让 SQLite 自己执行原始 SQL。
///
/// 这是一个与本项目全部代码无关的独立实现，因此可以用来判定
/// NaiveRecompute 与未来的真实引擎是否正确。
pub fn recompute_via_sqlite(
    schema: &Schema,
    query: &ViewQuery,
    base: &ZSet,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;
    conn.execute_batch(&schema.create_table_sql())
        .map_err(|e| EngineError(e.to_string()))?;

    let placeholders = vec!["?"; schema.arity()].join(", ");
    let insert_sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({})",
        schema.table,
        schema
            .column_names()
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders
    );

    {
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "基表状态含负权重 {weight}，行 {row:?}"
                )));
            }
            let bound: Vec<Bound> = row.0.iter().map(Bound).collect();
            let params: Vec<&dyn ToSql> = bound.iter().map(|b| b as &dyn ToSql).collect();
            for _ in 0..*weight {
                stmt.execute(params.as_slice())
                    .map_err(|e| EngineError(e.to_string()))?;
            }
        }
    }

    let sql = query.to_sql(schema);
    let mut stmt = conn.prepare(&sql).map_err(|e| EngineError(e.to_string()))?;
    let arity = query.output_arity();

    let mut out = ZSet::new();
    let mut rows = stmt.query([]).map_err(|e| EngineError(e.to_string()))?;
    while let Some(r) = rows.next().map_err(|e| EngineError(e.to_string()))? {
        let mut values = Vec::with_capacity(arity);
        for i in 0..arity {
            let raw = r.get_ref(i).map_err(|e| EngineError(e.to_string()))?;
            values.push(from_sqlite(raw)?);
        }
        out.update(Row::new(values), 1);
    }
    Ok(out)
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod oracle;
pub use oracle::recompute_via_sqlite;
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 24 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/oracle.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): 独立的 SQLite oracle"
```

---

## Task 9: 不变量断言

**Files:**
- Create: `crates/ivmlite-test/src/invariants.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: `ViewQuery`（Task 4）、`ivmlite_core::{Row, Value, ZSet}`
- Produces: `check_invariants(&ZSet, &ViewQuery) -> Result<(), String>`

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/invariants.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        }
    }

    fn out(region: &str, count: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(count)])
    }

    #[test]
    fn accepts_a_well_formed_state() {
        let z = ZSet::from_rows([(out("a", 1), 1), (out("b", 2), 1)]);
        assert!(check_invariants(&z, &q()).is_ok());
    }

    #[test]
    fn rejects_negative_weights() {
        let z = ZSet::from_rows([(out("a", 1), -1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("负权重"), "实得: {err}");
    }

    #[test]
    fn rejects_duplicate_group_keys() {
        // 同一个 group key "a" 出现了两行不同的聚合结果
        let z = ZSet::from_rows([(out("a", 1), 1), (out("a", 2), 1)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("group key"), "实得: {err}");
    }

    #[test]
    fn rejects_weight_greater_than_one_for_aggregate_views() {
        let z = ZSet::from_rows([(out("a", 1), 2)]);
        let err = check_invariants(&z, &q()).unwrap_err();
        assert!(err.contains("权重"), "实得: {err}");
    }
}
```

> `ZSet` 已经保证不会留 `w = 0` 的行（Task 2），因此不变量层不再重复检查僵尸行——那条性质由 `ZSet::update` 的单测覆盖。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test invariants`
Expected: 编译失败，`cannot find function check_invariants`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/invariants.rs` 顶部：

```rust
use std::collections::BTreeSet;

use ivmlite_core::{Row, Value, ZSet};

use crate::ViewQuery;

/// 不需要 oracle 就能检查的性质（spec §9.1 第一层）。
/// 跑得极快，因此在每一批 delta 之后都检查，而不是只在最后检查。
pub fn check_invariants(state: &ZSet, query: &ViewQuery) -> Result<(), String> {
    let key_arity = query.group_by.len();
    let mut seen: BTreeSet<Vec<Value>> = BTreeSet::new();

    for (row, weight) in state.iter() {
        if *weight < 0 {
            return Err(format!("最终状态出现负权重 {weight}，行 {row:?}"));
        }
        if *weight != 1 {
            return Err(format!(
                "聚合视图的每个 group 应恰好一行、权重为 1，实得权重 {weight}，行 {row:?}"
            ));
        }
        if row.len() != query.output_arity() {
            return Err(format!(
                "输出行宽度 {} 与视图的 {} 不符，行 {row:?}",
                row.len(),
                query.output_arity()
            ));
        }
        let key: Vec<Value> = (0..key_arity).map(|i| row.get(i).clone()).collect();
        if !seen.insert(key.clone()) {
            return Err(format!("group key {key:?} 在输出中出现多次"));
        }
    }
    Ok(())
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod invariants;
pub use invariants::check_invariants;
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 28 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/invariants.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): 不变量断言层"
```

---

## Task 10: 差分测试驱动与批次无关性

**Files:**
- Create: `crates/ivmlite-test/src/differential.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: 前面全部
- Produces: `Batching`（`All` / `One` / `Chunks(usize)`）、`TestCase { seed: u64, schema: Schema, query: ViewQuery, initial: Vec<Row>, ops: Vec<Op>, batching: Batching }`、`Failure { case_seed: u64, stage: String, detail: String }`、`run<E: Engine>(&mut E, &TestCase) -> Result<(), Failure>`、`gen_case(u64, &Schema, &Domain, usize, usize, Batching) -> TestCase`、`check_batch_invariance<E, F>(&TestCase, F) -> Result<(), Failure> where F: Fn() -> E`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/differential.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType, Domain, NaiveRecompute};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
            ],
        }
    }

    #[test]
    fn naive_engine_passes_every_enumerated_query() {
        let schema = schema();
        let domain = Domain::default();
        for (i, query) in crate::enumerate(&schema).into_iter().enumerate() {
            let mut case = gen_case(i as u64, &schema, &domain, 30, 200, Batching::Chunks(7));
            case.query = query;
            let mut engine = NaiveRecompute::new();
            run(&mut engine, &case).unwrap_or_else(|f| {
                panic!("seed {} 失败于 {}: {}", f.case_seed, f.stage, f.detail)
            });
        }
    }

    #[test]
    fn batch_invariance_holds_for_naive_engine() {
        let schema = schema();
        let domain = Domain::default();
        let case = gen_case(4242, &schema, &domain, 30, 200, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new).unwrap();
    }

    #[test]
    fn same_seed_produces_the_same_case() {
        let schema = schema();
        let domain = Domain::default();
        let a = gen_case(5, &schema, &domain, 10, 40, Batching::One);
        let b = gen_case(5, &schema, &domain, 10, 40, Batching::One);
        assert_eq!(a.initial, b.initial);
        assert_eq!(a.ops, b.ops);
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p ivmlite-test differential`
Expected: 编译失败，`cannot find function gen_case`

- [ ] **Step 3: 写实现**

`crates/ivmlite-test/src/differential.rs` 顶部：

```rust
use ivmlite_core::{Row, ZSet};
use rand::rngs::StdRng;
use rand::SeedableRng;

use crate::{
    check_invariants, enumerate, gen_ops, gen_rows, recompute_via_sqlite, Domain, Engine, Op,
    Schema, ViewQuery,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batching {
    /// 全部 delta 一次性应用
    All,
    /// 每条 delta 单独应用
    One,
    /// 每 n 条一批
    Chunks(usize),
}

#[derive(Debug, Clone)]
pub struct TestCase {
    pub seed: u64,
    pub schema: Schema,
    pub query: ViewQuery,
    pub initial: Vec<Row>,
    pub ops: Vec<Op>,
    pub batching: Batching,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub case_seed: u64,
    pub stage: String,
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[seed={}] {}: {}\n重放: cargo test -p ivmlite-test -- --exact <test> （seed 已固定）",
            self.case_seed, self.stage, self.detail
        )
    }
}

pub fn gen_case(
    seed: u64,
    schema: &Schema,
    domain: &Domain,
    initial_rows: usize,
    op_count: usize,
    batching: Batching,
) -> TestCase {
    let mut rng = StdRng::seed_from_u64(seed);
    let initial = gen_rows(&mut rng, schema, domain, initial_rows);
    let ops = gen_ops(&mut rng, schema, domain, &initial, op_count);
    let queries = enumerate(schema);
    let query = queries[seed as usize % queries.len()].clone();
    TestCase {
        seed,
        schema: schema.clone(),
        query,
        initial,
        ops,
        batching,
    }
}

fn batches(ops: &[Op], batching: Batching) -> Vec<ZSet> {
    let size = match batching {
        Batching::All => ops.len().max(1),
        Batching::One => 1,
        Batching::Chunks(n) => n.max(1),
    };
    ops.chunks(size)
        .map(|chunk| {
            let mut z = ZSet::new();
            for op in chunk {
                for (row, weight) in op.to_delta() {
                    z.update(row, weight);
                }
            }
            z
        })
        .collect()
}

fn initial_zset(initial: &[Row]) -> ZSet {
    ZSet::from_rows(initial.iter().cloned().map(|r| (r, 1)))
}

/// 跑完一个用例：逐批应用 delta，每批之后检查不变量，最后与 oracle 严格比对。
pub fn run<E: Engine>(engine: &mut E, case: &TestCase) -> Result<(), Failure> {
    let fail = |stage: &str, detail: String| Failure {
        case_seed: case.seed,
        stage: stage.to_string(),
        detail,
    };

    let mut base = initial_zset(&case.initial);
    engine
        .create_view(&case.schema, &case.query, &base)
        .map_err(|e| fail("create_view", e.to_string()))?;

    for (i, delta) in batches(&case.ops, case.batching).into_iter().enumerate() {
        engine
            .apply(&delta)
            .map_err(|e| fail(&format!("apply[{i}]"), e.to_string()))?;
        base.merge(&delta);

        let state = engine
            .materialize()
            .map_err(|e| fail(&format!("materialize[{i}]"), e.to_string()))?;
        check_invariants(&state, &case.query)
            .map_err(|e| fail(&format!("invariants[{i}]"), e))?;
    }

    let got = engine
        .materialize()
        .map_err(|e| fail("materialize[final]", e.to_string()))?;
    let want = recompute_via_sqlite(&case.schema, &case.query, &base)
        .map_err(|e| fail("oracle", e.to_string()))?;

    if got != want {
        return Err(fail(
            "diff",
            format!(
                "引擎与 oracle 不一致\n  query: {}\n  引擎: {:?}\n  oracle: {:?}",
                case.query.to_sql(&case.schema),
                got,
                want
            ),
        ));
    }
    Ok(())
}

/// spec §9.1 第二层：同一串 delta 无论怎么分批，最终状态必须一致。
/// 自动维护模式下无法测试这条性质，这是 v0 选择显式 refresh 的收益之一。
pub fn check_batch_invariance<E, F>(case: &TestCase, make: F) -> Result<(), Failure>
where
    E: Engine,
    F: Fn() -> E,
{
    let modes = [Batching::All, Batching::One, Batching::Chunks(3), Batching::Chunks(17)];
    let mut reference: Option<(Batching, ZSet)> = None;

    for mode in modes {
        let mut engine = make();
        let scoped = TestCase { batching: mode, ..case.clone() };
        run(&mut engine, &scoped)?;
        let state = engine.materialize().map_err(|e| Failure {
            case_seed: case.seed,
            stage: format!("batch_invariance[{mode:?}]"),
            detail: e.to_string(),
        })?;

        match &reference {
            None => reference = Some((mode, state)),
            Some((ref_mode, ref_state)) => {
                if *ref_state != state {
                    return Err(Failure {
                        case_seed: case.seed,
                        stage: "batch_invariance".into(),
                        detail: format!(
                            "{ref_mode:?} 与 {mode:?} 的最终状态不同\n  {ref_state:?}\n  {state:?}"
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod differential;
pub use differential::{check_batch_invariance, gen_case, run, Batching, Failure, TestCase};
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p ivmlite-test`
Expected: 31 passed

- [ ] **Step 5: 提交**

```bash
git add crates/ivmlite-test/src/differential.rs crates/ivmlite-test/src/lib.rs
git commit -m "feat(test): 差分测试驱动与批次无关性检查"
```

---

## Task 11: 植入 bug 的引擎 + shrinker + M0 完成判定

**Files:**
- Create: `crates/ivmlite-test/src/buggy.rs`, `crates/ivmlite-test/src/shrink.rs`
- Create: `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `crates/ivmlite-test/src/lib.rs`

**Interfaces:**
- Consumes: 前面全部
- Produces: `NoRetractionEngine::new() -> NoRetractionEngine`（实现 `Engine`）、`shrink<E, F>(&TestCase, F) -> TestCase where E: Engine, F: Fn() -> E`

> **这是 M0 的完成判定。** 不验证"测试框架真的会红"，后续拿到的绿全是假绿。

- [ ] **Step 1: 写植入 bug 的引擎**

`crates/ivmlite-test/src/buggy.rs`：

```rust
use ivmlite_core::ZSet;

use crate::{Engine, EngineError, NaiveRecompute, Schema, ViewQuery};

/// 故意植入 bug 的引擎：聚合结果变化时**只发出新行、不撤回旧行**。
///
/// 这正是 spec §6.2 所说的 IVM 头号 bug 来源。它的存在不是为了被修好，
/// 而是为了证明测试框架抓得住它。
#[derive(Debug, Default)]
pub struct NoRetractionEngine {
    inner: NaiveRecompute,
    accumulated: ZSet,
}

impl NoRetractionEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Engine for NoRetractionEngine {
    fn create_view(
        &mut self,
        schema: &Schema,
        query: &ViewQuery,
        initial: &ZSet,
    ) -> Result<(), EngineError> {
        self.inner.create_view(schema, query, initial)?;
        self.accumulated = self.inner.materialize()?;
        Ok(())
    }

    fn apply(&mut self, delta: &ZSet) -> Result<(), EngineError> {
        self.inner.apply(delta)?;
        // BUG（有意为之）：把新的聚合结果并进来，却从不撤回上一次发出的行。
        let fresh = self.inner.materialize()?;
        for (row, weight) in fresh.iter() {
            self.accumulated.update(row.clone(), *weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        Ok(self.accumulated.clone())
    }
}
```

- [ ] **Step 2: 写 shrinker**

`crates/ivmlite-test/src/shrink.rs`：

```rust
use ivmlite_core::Row;

use crate::{run, Engine, Op, TestCase};

fn still_fails<E, F>(case: &TestCase, make: &F) -> bool
where
    E: Engine,
    F: Fn() -> E,
{
    let mut engine = make();
    run(&mut engine, case).is_err()
}

/// 序列的合法性：每个 DELETE / UPDATE 必须命中当时存在的行。
///
/// 这是自研 shrinker 而非直接用 proptest 的原因——朴素的缩小会删掉某个
/// INSERT，让后续针对该行的 DELETE 悬空，产出一个引擎本就不该处理的非法
/// 序列，于是"失败"变得毫无意义（spec §9.3）。
fn is_legal(initial: &[Row], ops: &[Op]) -> bool {
    let mut live: Vec<Row> = initial.to_vec();
    for op in ops {
        match op {
            Op::Insert(r) => live.push(r.clone()),
            Op::Delete(r) => match live.iter().position(|x| x == r) {
                Some(i) => {
                    live.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match live.iter().position(|x| x == old) {
                Some(i) => {
                    live.swap_remove(i);
                    live.push(new.clone());
                }
                None => return false,
            },
        }
    }
    true
}

/// 把失败用例缩到最小：先缩更新序列（delta-debugging），再缩初始数据。
/// 每一步都要求缩小后的用例**仍然合法且仍然失败**。
pub fn shrink<E, F>(case: &TestCase, make: F) -> TestCase
where
    E: Engine,
    F: Fn() -> E,
{
    let mut best = case.clone();

    // 阶段一：按 delta-debugging 的粒度递减删除 op 区间。
    let mut granularity = best.ops.len().max(1);
    while granularity >= 1 {
        let mut improved = true;
        while improved {
            improved = false;
            let chunk = (best.ops.len() / granularity).max(1);
            let mut start = 0;
            while start < best.ops.len() {
                let end = (start + chunk).min(best.ops.len());
                let mut ops = best.ops.clone();
                ops.drain(start..end);

                if is_legal(&best.initial, &ops) {
                    let candidate = TestCase { ops, ..best.clone() };
                    if still_fails(&candidate, &make) {
                        best = candidate;
                        improved = true;
                        continue; // 不推进 start，同一位置继续尝试
                    }
                }
                start = end;
            }
        }
        if granularity == 1 {
            break;
        }
        granularity /= 2;
    }

    // 阶段二：逐条删除初始行。
    let mut i = 0;
    while i < best.initial.len() {
        let mut initial = best.initial.clone();
        initial.remove(i);
        if is_legal(&initial, &best.ops) {
            let candidate = TestCase { initial, ..best.clone() };
            if still_fails(&candidate, &make) {
                best = candidate;
                continue; // 不推进 i
            }
        }
        i += 1;
    }

    best
}
```

`crates/ivmlite-test/src/lib.rs` 追加：

```rust
mod buggy;
mod shrink;
pub use buggy::NoRetractionEngine;
pub use shrink::shrink;
```

- [ ] **Step 3: 写 M0 完成判定的集成测试**

`crates/ivmlite-test/tests/harness_catches_bugs.rs`：

```rust
use ivmlite_test::{
    check_batch_invariance, gen_case, run, shrink, Batching, Column, ColumnType, Domain,
    NaiveRecompute, NoRetractionEngine, Schema,
};

fn schema() -> Schema {
    Schema {
        table: "orders".into(),
        columns: vec![
            Column { name: "region".into(), ty: ColumnType::Text, nullable: true },
            Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
        ],
    }
}

#[test]
fn naive_engine_is_green_across_many_seeds() {
    let schema = schema();
    let domain = Domain::default();
    for seed in 0..50 {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NaiveRecompute::new();
        run(&mut engine, &case)
            .unwrap_or_else(|f| panic!("参照实现不应失败: {f}"));
    }
}

#[test]
fn naive_engine_satisfies_batch_invariance() {
    let schema = schema();
    let domain = Domain::default();
    for seed in 0..10 {
        let case = gen_case(seed, &schema, &domain, 25, 120, Batching::All);
        check_batch_invariance(&case, NaiveRecompute::new)
            .unwrap_or_else(|f| panic!("参照实现不应违反批次无关性: {f}"));
    }
}

/// M0 完成判定其一：框架必须抓到植入的 bug。
#[test]
fn harness_catches_the_missing_retraction_bug() {
    let schema = schema();
    let domain = Domain::default();
    let mut caught = 0;
    for seed in 0..50 {
        let case = gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5));
        let mut engine = NoRetractionEngine::new();
        if run(&mut engine, &case).is_err() {
            caught += 1;
        }
    }
    assert!(
        caught >= 45,
        "50 个 seed 中只抓到 {caught} 个——生成器的 bug 检出率过低，\
         说明值域或有偏采样的参数需要调整"
    );
}

/// M0 完成判定其二：失败用例必须能缩到 10 步以内。
#[test]
fn failing_case_shrinks_to_under_ten_ops() {
    let schema = schema();
    let domain = Domain::default();

    let case = (0..50)
        .map(|seed| gen_case(seed, &schema, &domain, 25, 150, Batching::Chunks(5)))
        .find(|c| {
            let mut engine = NoRetractionEngine::new();
            run(&mut engine, c).is_err()
        })
        .expect("应当至少有一个失败用例");

    let minimal = shrink(&case, NoRetractionEngine::new);

    let mut engine = NoRetractionEngine::new();
    assert!(run(&mut engine, &minimal).is_err(), "缩小后必须仍然失败");
    assert!(
        minimal.ops.len() <= 10,
        "spec §11 M0 要求缩到 10 步以内，实得 {} 步",
        minimal.ops.len()
    );
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p ivmlite-test --test harness_catches_bugs -- --nocapture`
Expected: 4 passed

若 `harness_catches_the_missing_retraction_bug` 的检出率不足，**不要放宽断言**——调 `Domain::distinct`（更小）或 `gen_ops` 的删改比例（更高）。检出率低说明生成器没有制造出足够的 group 复用，这正是 spec §9.2 警告的失败模式。

- [ ] **Step 5: 跑全量并提交**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/ivmlite-test/src/buggy.rs crates/ivmlite-test/src/shrink.rs \
        crates/ivmlite-test/src/lib.rs crates/ivmlite-test/tests/harness_catches_bugs.rs
git commit -m "feat(test): 植入 bug 的引擎、合法性保持的 shrinker 与 M0 完成判定"
```

---

## Task 12: Benchmark harness 与三条 same-host 基线

**Files:**
- Create: `crates/ivmlite-bench/Cargo.toml`, `crates/ivmlite-bench/src/main.rs`, `crates/ivmlite-bench/src/baseline.rs`
- Modify: `Cargo.toml`（把 `ivmlite-bench` 加回 members）

**Interfaces:**
- Consumes: `ivmlite-test` 的 `Schema` / `ViewQuery` / `Domain` / `gen_rows` / `gen_ops` / `Engine` / `NaiveRecompute`
- Produces: 可执行文件 `ivmlite-bench`，向 stdout 输出 CSV：`baseline,views,base_rows,batch_size,apply_ms,write_amp_ms`

- [ ] **Step 1: 建 crate 清单**

`crates/ivmlite-bench/Cargo.toml`：

```toml
[package]
name = "ivmlite-bench"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
ivmlite-core.workspace = true
ivmlite-test = { path = "../ivmlite-test" }
rusqlite.workspace = true
rand.workspace = true
```

并把 `ivmlite-bench` 加回根 `Cargo.toml` 的 `members`。

- [ ] **Step 2: 写三条基线**

`crates/ivmlite-bench/src/baseline.rs`：

```rust
use ivmlite_core::{Row, Value};
use ivmlite_test::{Op, Schema, ViewQuery};
use rusqlite::{Connection, ToSql};

/// spec §10.2 的三条 same-host 对照组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// 下界：只写基表，完全不维护视图。纯写入成本。
    NoMaintenance,
    /// 怀疑者：手写 trigger 维护汇总表。v0 引擎必须打赢它，否则没有故事。
    HandWrittenTrigger,
    /// 基线：每批 delta 之后把所有视图 SQL 重跑一遍。交叉点在这里测量。
    NaiveRecompute,
}

impl Baseline {
    pub fn label(self) -> &'static str {
        match self {
            Baseline::NoMaintenance => "no_maintenance",
            Baseline::HandWrittenTrigger => "hand_written_trigger",
            Baseline::NaiveRecompute => "naive_recompute",
        }
    }
}

fn bind(row: &Row) -> Vec<rusqlite::types::Value> {
    row.0
        .iter()
        .map(|v| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::Int(n) => rusqlite::types::Value::Integer(*n),
            Value::Text(s) => rusqlite::types::Value::Text(s.clone()),
        })
        .collect()
}

/// 为一个视图装上手写的 trigger 维护逻辑。
///
/// 只支持 `GROUP BY <单列> → SUM(<列>), COUNT(*)`，这正是 v0 的形状。
/// 这个函数刻意写得很直接——它代表"一个有经验的工程师不用任何 IVM 框架
/// 会怎么做"，是 v0 必须超越的基准。
pub fn install_hand_written_trigger(
    conn: &Connection,
    schema: &Schema,
    view: &str,
    group_col: &str,
    sum_col: &str,
) -> rusqlite::Result<()> {
    let t = &schema.table;
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE "{view}" (
            k ANY,
            s INTEGER NOT NULL,
            c INTEGER NOT NULL,
            PRIMARY KEY (k)
        ) STRICT;

        CREATE TRIGGER "{view}_ins" AFTER INSERT ON "{t}" BEGIN
            INSERT INTO "{view}"(k, s, c) VALUES (NEW."{group_col}", NEW."{sum_col}", 1)
            ON CONFLICT(k) DO UPDATE SET s = s + NEW."{sum_col}", c = c + 1;
        END;

        CREATE TRIGGER "{view}_del" AFTER DELETE ON "{t}" BEGIN
            UPDATE "{view}" SET s = s - OLD."{sum_col}", c = c - 1 WHERE k IS OLD."{group_col}";
            DELETE FROM "{view}" WHERE k IS OLD."{group_col}" AND c = 0;
        END;
        "#
    ))
}

/// 应用一批变更，返回写入基表本身消耗的毫秒数（即写放大的度量基准）。
pub fn apply_ops(conn: &Connection, schema: &Schema, ops: &[Op]) -> rusqlite::Result<f64> {
    let cols: Vec<String> = schema
        .column_names()
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect();
    let placeholders = vec!["?"; schema.arity()].join(", ");
    let insert = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({})",
        schema.table,
        cols.join(", "),
        placeholders
    );
    let predicate = cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{c} IS ?{}", i + 1))
        .collect::<Vec<_>>()
        .join(" AND ");
    let delete = format!(
        "DELETE FROM \"{}\" WHERE rowid = (SELECT rowid FROM \"{}\" WHERE {} LIMIT 1)",
        schema.table, schema.table, predicate
    );

    let start = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    for op in ops {
        for (row, weight) in op.to_delta() {
            let values = bind(&row);
            let params: Vec<&dyn ToSql> = values.iter().map(|v| v as &dyn ToSql).collect();
            if weight > 0 {
                tx.execute(&insert, params.as_slice())?;
            } else {
                tx.execute(&delete, params.as_slice())?;
            }
        }
    }
    tx.commit()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// 朴素重跑：把所有视图 SQL 各跑一遍，返回耗时毫秒。
pub fn recompute_all(
    conn: &Connection,
    schema: &Schema,
    views: &[ViewQuery],
) -> rusqlite::Result<f64> {
    let start = std::time::Instant::now();
    for q in views {
        let sql = q.to_sql(schema);
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
```

- [ ] **Step 3: 写矩阵驱动**

`crates/ivmlite-bench/src/main.rs`：

```rust
mod baseline;

use baseline::{apply_ops, install_hand_written_trigger, recompute_all, Baseline};
use ivmlite_test::{enumerate, gen_ops, gen_rows, Column, ColumnType, Domain, Op, Schema};
use rand::rngs::StdRng;
use rand::SeedableRng;
use rusqlite::Connection;

const VIEW_COUNTS: [usize; 4] = [1, 10, 50, 200];
const BASE_ROWS: [usize; 3] = [10_000, 100_000, 1_000_000];
const BATCH_SIZES: [usize; 4] = [1, 10, 100, 1000];

fn schema() -> Schema {
    Schema {
        table: "orders".into(),
        columns: vec![
            Column { name: "region".into(), ty: ColumnType::Text, nullable: false },
            Column { name: "amount".into(), ty: ColumnType::Integer, nullable: false },
        ],
    }
}

fn seed_table(conn: &Connection, schema: &Schema, rows: usize, rng: &mut StdRng) -> rusqlite::Result<()> {
    conn.execute_batch(&schema.create_table_sql())?;
    // 基表用宽值域：benchmark 关心的是规模，不是 group 复用。
    let domain = Domain { distinct: 1000, null_rate: 0.0 };
    let generated = gen_rows(rng, schema, &domain, rows);
    let ops: Vec<Op> = generated.into_iter().map(Op::Insert).collect();
    apply_ops(conn, schema, &ops)?;
    Ok(())
}

fn main() -> rusqlite::Result<()> {
    println!("baseline,views,base_rows,batch_size,apply_ms,maintain_ms");

    let schema = schema();
    let all_queries = enumerate(&schema);

    for baseline in [
        Baseline::NoMaintenance,
        Baseline::HandWrittenTrigger,
        Baseline::NaiveRecompute,
    ] {
        for views in VIEW_COUNTS {
            // 手写 trigger 基线只有单一形状，超过 1 个视图就等比例复制。
            let queries: Vec<_> = all_queries.iter().cycle().take(views).cloned().collect();

            for base_rows in BASE_ROWS {
                for batch_size in BATCH_SIZES {
                    let mut rng = StdRng::seed_from_u64(0xB0BA);
                    let conn = Connection::open_in_memory()?;
                    seed_table(&conn, &schema, base_rows, &mut rng)?;

                    if baseline == Baseline::HandWrittenTrigger {
                        for i in 0..views {
                            install_hand_written_trigger(
                                &conn,
                                &schema,
                                &format!("mv_{i}"),
                                "region",
                                "amount",
                            )?;
                        }
                    }

                    let domain = Domain { distinct: 1000, null_rate: 0.0 };
                    let existing = gen_rows(&mut rng, &schema, &domain, 0);
                    let ops = gen_ops(&mut rng, &schema, &domain, &existing, batch_size);

                    let apply_ms = apply_ops(&conn, &schema, &ops)?;
                    let maintain_ms = match baseline {
                        Baseline::NoMaintenance => 0.0,
                        // trigger 的成本已经计入 apply_ms——这正是"写放大"
                        Baseline::HandWrittenTrigger => 0.0,
                        Baseline::NaiveRecompute => recompute_all(&conn, &schema, &queries)?,
                    };

                    println!(
                        "{},{},{},{},{:.3},{:.3}",
                        baseline.label(),
                        views,
                        base_rows,
                        batch_size,
                        apply_ms,
                        maintain_ms
                    );
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: 跑一次缩小规模的 smoke run**

先把 `BASE_ROWS` 临时改成 `[1_000]`、`VIEW_COUNTS` 改成 `[1, 10]` 跑通，确认 CSV 成形、无 panic：

Run: `cargo run -p ivmlite-bench --release | head -20`
Expected: CSV 表头 + 若干行数据，`no_maintenance` 的 `maintain_ms` 恒为 0，`naive_recompute` 的 `maintain_ms` 随 `base_rows` 明显增长。

改回完整常量后再跑一次完整矩阵，把结果存到 `docs/bench/2026-XX-XX-m0-baseline.csv`。

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml crates/ivmlite-bench docs/bench
git commit -m "feat(bench): benchmark 矩阵与三条 same-host 基线"
```

---

## Self-Review

**1. Spec 覆盖**

| Spec 要求 | 落点 |
|---|---|
| §4.2 core 不依赖 rusqlite | Task 1 Step 1（空依赖表）+ Global Constraints |
| §5.1 权重不变量 | Task 2（`ZSet` 归零即删）、Task 9（负权重、重复 group key） |
| §5.1 Value 无浮点 | Task 1（枚举只有三个变体）、Task 4（SUM 只作用于 INTEGER） |
| §6.2 retraction 语义 | Task 11（`NoRetractionEngine` 就是它的反面教材） |
| §7.1 STRICT table | Task 3（`create_table_sql` 断言 STRICT） |
| §7.1 裸列 group-by key | Task 4（`group_by: Vec<usize>` 只能是列下标） |
| §8.1 UPDATE = retract + insert | Task 6（`Op::to_delta`） |
| §9.1 四层验证 | 不变量→Task 9；批次无关性→Task 10；主 oracle→Task 8；交叉验证→M2（不在 M0 范围，spec 已如此规定） |
| §9.2 窄值域 / 高 NULL / 有偏采样 / query 穷举 | Task 5、Task 6、Task 4 |
| §9.3 保持合法性的 shrinking | Task 11（`is_legal` 门禁） |
| §9.4 seed 可复现 | Task 5、6、10 各有一条 reproducibility 测试 |
| §10.2 三条 same-host 对照组 | Task 12 |
| §10.5 写放大 | Task 12（trigger 成本计入 `apply_ms`，与 `no_maintenance` 相减即得） |
| §11 M0 完成判定 | Task 11 Step 3 的两个测试 |

**已知缺口（有意为之，非遗漏）：**
- **空间放大与 bootstrap 耗时**（spec §10.5）未在 M0 度量——两者都需要真实引擎才有意义，M1 补。
- **`criterion` 微基准**（spec §10）未引入——M0 的主 benchmark 是端到端矩阵，微基准等 core 有算子可测时再加。
- **`ivmlite-sql` / `ivmlite-sqlite` crate** 未创建，见 Global Constraints 末条。

**2. Placeholder 扫描**：无 TBD / TODO；每个代码步骤都给了可直接粘贴的完整实现；Task 间未出现未定义的类型或函数。

**3. 类型一致性核对**：`Engine::materialize` 全程为 `&mut self`（engine.rs / naive.rs / buggy.rs / differential.rs 一致）；`Domain` 字段 `distinct` / `null_rate` 在 data.rs、differential.rs、main.rs 中拼写一致；`Op::to_delta` 返回 `Vec<(Row, i64)>`，在 differential.rs 与 baseline.rs 中按此消费；`Failure` 三字段 `case_seed` / `stage` / `detail` 在构造与 `Display` 中一致。

---

## 执行顺序说明

Task 1 → 12 有严格依赖，不可并行乱序。Task 11 是 M0 的验收关口——它红着，M0 就没完成，**不允许放宽该测试的断言来让它变绿**（spec §11 明确了这一点的理由）。
