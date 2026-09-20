# M1a Phase 1：多表框架重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M0 的差分测试框架从"一张表"改造成"多张表"，并把 plan IR 类型族迁进 `ivmlite-core`，使 M1a 的引擎有地方可写。

**Architecture:** 两件事。其一，`Schema` / `ViewQuery` 族目前住在 `ivmlite-test`，但引擎要住 `ivmlite-core`，而 core 不得依赖 test——这些类型本质上就是 v0 的 plan IR，必须迁下去；SQL 渲染（`create_table_sql` / `to_sql`）留在 test，因为它只为驱动 oracle 而存在。其二，`TestCase` / oracle / 生成器 / 驱动全部按表索引化，单表用例变成"只有一张表的多表用例"。

**Tech Stack:** Rust 1.95、`rusqlite`（bundled）、`rand` 0.10（`RngExt`）、`serde`（core 的可选 feature）。

**Spec:** [`docs/superpowers/specs/2026-09-18-ivmlite-design.md`](../specs/2026-09-18-ivmlite-design.md)

## 本计划的范围与不在范围

**在范围**：变异门禁审计、plan IR 类型迁移、`Database` 类型、多表 oracle、多表生成器、多表驱动。

**不在范围**：任何算子、`Arrangement`、consolidation、join。**引擎的计划在本计划落地之后另写**——那些任务的代码必须写在本计划产出的真实类型上，现在写等于对着猜的类型写，而 M0 的两个编译级缺陷正是这么来的。

完成时的状态：M0 的全部测试在"只有一张表的 database"下仍绿，且框架已能表达多表用例。**引擎仍不存在**，这是预期的。

## Global Constraints

- **`ivmlite-core` 不得依赖 `rusqlite` / `libsqlite3-sys`**（spec §4.2）。迁进去的类型必须是纯数据 + 纯方法。
- **SQL 渲染不进 core。** `create_table_sql`、`to_sql` 留在 `ivmlite-test`，理由是它们只为驱动 oracle 而存在；core 拥有 IR，test 拥有"如何把这个 IR 表达成 SQLite 能执行的 SQL"。
- **零 `unsafe`**（spec §4.2：`unsafe` 只允许出现在尚未创建的 `ivmlite-sqlite`）。
- **差分测试的 schema 固定为每表 2 列**（spec §9.2 第 4 条）。这是"穷举优于随机"成立的前提，不是魔数——加宽到 3 列会让穷举规模涨约 8 倍。不得为了"覆盖更全"擅自加宽。
- **`Value` 只有 `Null` / `Int` / `Text`**（spec §5.1）。
- **权重不变量**（spec §5.1）：最终状态无负权重；归零即删，不留僵尸行。
- **`rand` 0.10**：`random_range` / `random_bool` 在 `RngExt` 上，不在 `Rng` 上。`use rand::RngExt;`。
- **每新增一条 spec 强制的不变量，必须在 [`docs/mutation-gates.md`](../../mutation-gates.md) 登记一行，且"已验证"栏必须是真跑过变异的。** 跑全套约 1.5 秒；不得以"跑测试贵"为由跳过——那个假设本项目已经错过一次。
- 迁移与重构期间，**M0 的验收阈值不得退化**：`harness_catches_the_missing_retraction_bug` 仍须 ≥90% 检出，`failing_case_shrinks_to_under_ten_ops` 仍须 ≤10 步。退化说明改动动了不该动的东西，**不许放宽断言**。

---

## File Structure

```
crates/ivmlite-core/src/
  lib.rs            + pub use schema::*, query::*, database::*
  schema.rs         新：ColumnType / Column / Schema（arity, column_names）
  query.rs          新：AggFn / Agg / Predicate / ViewQuery（output_arity）
  database.rs       新：Database
crates/ivmlite-test/src/
  sql.rs            新：create_table_sql / view_query_to_sql（从 core 类型渲染 SQL）
  schema.rs         删除（类型已迁走）
  query.rs          只留 enumerate（生成属测试职责）
  oracle.rs         改：多表签名
  data.rs           改：按表生成
  ops.rs            改：带表标签、按表维护 live 集合
  differential.rs   改：TestCase / run / check_batch_invariance 多表化
  engine.rs         改：create_view 收 &Database 与按表的 initial
  naive.rs          改：按表持有 base
  buggy.rs          改：跟随 naive
```

---

## Task 1: 变异门禁审计

**Files:**
- Modify: `docs/mutation-gates.md`
- Modify: 审计中发现缺口的任何测试文件

**Interfaces:**
- Consumes: 现有全部测试
- Produces: 无新接口；产出是一份"已验证"栏全部为真的门禁表，以及补上的缺口测试

> **为什么这是第一个任务**：本计划后面的每一步都在重构 M0 的代码，而重构的安全网就是那些测试。**先确认安全网真的接得住，再开始拆。** 门禁表里有 25 条标着"未验证"——它们都有具名的守护测试，但没人真的把实现改坏跑过一遍。M0 的最终评审正是用这个方法在约一秒一次的运行里找到三处"测试在、但抓不住"。

- [ ] **Step 1: 逐条跑完 25 条未验证的变异**

对 `docs/mutation-gates.md` 中"已验证"栏为"未验证"的每一行：

1. 按"变异"栏所述把实现改坏
2. **先确认改坏后的代码仍能编译** —— 这一步不能省。若变异引入语法错误，`cargo test` 的失败是编译失败而非断言失败，而 `grep FAILED` 看不出区别，会把一次空验证记成有效。确认方式：`cargo build --workspace --locked` 成功。
3. 跑 `cargo test --workspace --locked`，记录哪个测试红了
4. 还原（`git checkout -- <file>`），确认 `git status` 干净且测试恢复全绿

- [ ] **Step 2: 把结果填回门禁表**

红了预期的测试 → "已验证"栏改为 `**已验证**`。

**没红，或红的是别的测试** → 这是一处真缺口。不要改门禁表去迁就现状，也不要放宽任何断言。补一个会红的测试，再把该行标为已验证，并在报告里单独列出这一条。

- [ ] **Step 3: 更新统计并提交**

文件末尾的统计数字**必须由脚本从本文件数出**，不得手写——上一版手写的第一次就错了（12/26 实为 13/25）。

```bash
python3 - <<'PY'
s=open('docs/mutation-gates.md').read()
rows=[l for l in s.splitlines() if l.startswith('| §')]
print(f"总 {len(rows)} 行：已验证 {sum('**已验证**' in l for l in rows)}，"
      f"未验证 {sum('未验证' in l for l in rows)}，"
      f"不适用 {sum(l.rstrip().endswith('| — |') for l in rows)}")
PY
```

```bash
git add docs/mutation-gates.md crates/
git commit -m "test: 变异门禁审计——把未验证的条目逐条跑实"
```

---

## Task 2: plan IR 类型族迁进 `ivmlite-core`

**Files:**
- Create: `crates/ivmlite-core/src/schema.rs`, `crates/ivmlite-core/src/query.rs`
- Create: `crates/ivmlite-test/src/sql.rs`
- Delete: `crates/ivmlite-test/src/schema.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-test/src/lib.rs`, `crates/ivmlite-test/src/query.rs`（只留 `enumerate`）
- Modify: 全部引用这些类型的文件（`oracle.rs` / `data.rs` / `ops.rs` / `differential.rs` / `engine.rs` / `naive.rs` / `buggy.rs` / `tests/harness_catches_bugs.rs`）

**Interfaces:**
- Consumes: 无新依赖
- Produces: `ivmlite_core::{ColumnType, Column, Schema, AggFn, Agg, Predicate, ViewQuery}`，方法 `Schema::arity()`、`Schema::column_names()`、`ViewQuery::output_arity()`；`ivmlite_test::sql::{create_table_sql, view_query_to_sql}`

> **为什么必须迁**：引擎要住 `ivmlite-core`，而 `Engine::create_view` 的签名里有 `&Schema` 和 `&ViewQuery`。这两个类型现在住在 `ivmlite-test`，于是引擎要么依赖 test（依赖方向反了，spec §4.2 禁止），要么住在 test 里（那它就不是产品了）。这些类型本质上就是 v0 的 plan IR——`ViewQuery { group_by, aggs, predicate }` 正是 spec §5.2 那个 `Plan` 的 v0 子集的扁平形式。

- [ ] **Step 1: 迁移类型，保持行为不变**

`crates/ivmlite-core/src/schema.rs`：把 `ivmlite-test/src/schema.rs` 的 `ColumnType` / `Column` / `Schema` 原样搬过来，**但不要搬 `create_table_sql`**——它连同其单测一起去 `ivmlite-test/src/sql.rs`。`Schema` 保留 `arity()` 与 `column_names()`。

`crates/ivmlite-core/src/query.rs`：搬 `AggFn` / `Agg` / `Predicate` / `ViewQuery` 与 `output_arity()`，**不要搬 `to_sql`，也不要搬 `enumerate`**。

两个文件的类型都要保留原有的 derive，并加上 serde 的条件 derive，与 `Value` / `Row` 一致：

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
```

`crates/ivmlite-test/src/sql.rs`：

```rust
//! 把 core 的 plan IR 渲染成 SQLite 能执行的 SQL。
//!
//! 这一层刻意留在 `ivmlite-test` 而不进 `ivmlite-core`：它存在的唯一理由是
//! 驱动 oracle（让 SQLite 自己算一遍当作权威判据）。core 拥有 IR，test 拥有
//! 「如何把这个 IR 表达成 SQL」。

use ivmlite_core::{AggFn, ColumnType, Predicate, Schema, ViewQuery};

fn sql_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
    }
}

/// 生成 STRICT 建表语句（spec §7.1：STRICT 把列类型钉死，否则 type affinity
/// 会让一列逐行存不同类型，把一个逻辑上的 group key 拆成两个）。
pub fn create_table_sql(schema: &Schema) -> String {
    let cols: Vec<String> = schema
        .columns
        .iter()
        .map(|c| {
            let null = if c.nullable { "" } else { " NOT NULL" };
            format!("\"{}\" {}{}", c.name, sql_type(c.ty), null)
        })
        .collect();
    format!(
        "CREATE TABLE \"{}\" ({}) STRICT",
        schema.table,
        cols.join(", ")
    )
}

/// 把 ViewQuery 渲染成 SQL。列下标按 `schema` 解析。
pub fn view_query_to_sql(query: &ViewQuery, schema: &Schema) -> String {
    let name = |i: usize| format!("\"{}\"", schema.columns[i].name);

    let mut select: Vec<String> = query.group_by.iter().map(|i| name(*i)).collect();
    for agg in &query.aggs {
        select.push(match (agg.func, agg.column) {
            (AggFn::Count, _) => "COUNT(*)".to_string(),
            (AggFn::Sum, Some(i)) => format!("SUM({})", name(i)),
            (AggFn::Sum, None) => panic!("SUM 必须指定列"),
        });
    }

    let where_clause = match &query.predicate {
        Predicate::None => String::new(),
        Predicate::IntGt { column, value } => format!(" WHERE {} > {}", name(*column), value),
        Predicate::IsNotNull { column } => format!(" WHERE {} IS NOT NULL", name(*column)),
    };

    let group = query
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
```

把 `create_table_sql` 与 `to_sql` 的原有单测一并搬进 `sql.rs` 的测试模块，改为调用自由函数。

- [ ] **Step 2: 修依赖与导出**

`crates/ivmlite-core/Cargo.toml` 不变（仍然零必需依赖）。

`crates/ivmlite-core/src/lib.rs`：

```rust
mod query;
mod row;
mod schema;
mod value;
mod zset;

pub use query::{Agg, AggFn, Predicate, ViewQuery};
pub use row::Row;
pub use schema::{Column, ColumnType, Schema};
pub use value::Value;
pub use zset::ZSet;
```

`crates/ivmlite-test/src/lib.rs`：删掉 `mod schema;` 与它的 `pub use`，加 `mod sql;` 与 `pub use sql::{create_table_sql, view_query_to_sql};`。为了让现有调用点少改，**从 test 里重新导出 core 的这些类型**：

```rust
pub use ivmlite_core::{Agg, AggFn, Column, ColumnType, Predicate, Schema, ViewQuery};
```

- [ ] **Step 3: 修全部调用点**

`schema.create_table_sql()` → `create_table_sql(&schema)`；`query.to_sql(&schema)` → `view_query_to_sql(&query, &schema)`。改完跑：

Run: `cargo build --workspace --locked`
Expected: 编译通过。

- [ ] **Step 4: 确认行为零变化**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: **测试总数与 Task 1 结束时完全相同，全部通过。** 这是纯迁移，任何测试数变化都说明有东西被漏搬或被改了行为。

- [ ] **Step 5: 变异验证迁移没有削弱守护**

迁移最容易悄悄丢掉的是"STRICT"这类断言。跑两条变异确认它们跟着搬过来了：

1. 在 `create_table_sql` 中去掉 `STRICT` 后缀 → 期望 `create_table_sql_is_strict` 红
2. 让 `view_query_to_sql` 的 `Count` 分支输出 `COUNT(1)` → 期望 `to_sql` 的精确字符串测试红

两条都要先确认改坏后能编译。还原后确认全绿。

- [ ] **Step 6: 提交**

```bash
git add crates/
git commit -m "refactor(core): plan IR 类型族迁入 ivmlite-core，SQL 渲染留在 ivmlite-test"
```

---

## Task 3: `Database` 类型与多表 oracle

**Files:**
- Create: `crates/ivmlite-core/src/database.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`, `crates/ivmlite-test/src/oracle.rs`

**Interfaces:**
- Consumes: `Schema`（Task 2）
- Produces: `ivmlite_core::Database`，方法 `single(Schema) -> Database`、`tables(&self) -> &[Schema]`、`get(&self, &str) -> Option<&Schema>`、`len()`、`is_empty()`；`recompute_via_sqlite(&Database, &ViewQuery, &BTreeMap<String, ZSet>) -> Result<ZSet, EngineError>`

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/database.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, ColumnType};

    fn s(name: &str) -> Schema {
        Schema {
            table: name.into(),
            columns: vec![Column {
                name: "a".into(),
                ty: ColumnType::Integer,
                nullable: false,
            }],
        }
    }

    #[test]
    fn single_wraps_one_schema() {
        let db = Database::single(s("orders"));
        assert_eq!(db.len(), 1);
        assert_eq!(db.get("orders").map(|x| x.table.as_str()), Some("orders"));
    }

    #[test]
    fn get_returns_none_for_unknown_table() {
        assert!(Database::single(s("orders")).get("nope").is_none());
    }

    #[test]
    fn table_order_is_preserved() {
        let db = Database::new(vec![s("b"), s("a")]);
        let names: Vec<&str> = db.tables().iter().map(|t| t.table.as_str()).collect();
        assert_eq!(names, vec!["b", "a"], "顺序必须保留——失败用例要凭 seed 精确重放");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core database`
Expected: 编译失败，`cannot find type Database`

- [ ] **Step 3: 写实现**

```rust
use crate::Schema;

/// 差分用例涉及的全部基表。
///
/// 用 `Vec` 而非 `HashMap`：表的数量是个位数，按名查找的线性扫描无关紧要，
/// 而顺序确定是硬要求——失败用例必须能凭 seed 精确重放（spec §9.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Database {
    tables: Vec<Schema>,
}

impl Database {
    pub fn new(tables: Vec<Schema>) -> Self {
        Database { tables }
    }

    /// 单表用例就是只有一张表的多表用例。
    pub fn single(schema: Schema) -> Self {
        Database { tables: vec![schema] }
    }

    pub fn tables(&self) -> &[Schema] {
        &self.tables
    }

    pub fn get(&self, table: &str) -> Option<&Schema> {
        self.tables.iter().find(|s| s.table == table)
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}
```

`lib.rs` 加 `mod database;` 与 `pub use database::Database;`。

- [ ] **Step 4: 多表 oracle**

改 `crates/ivmlite-test/src/oracle.rs` 的签名与表建立逻辑：

```rust
/// 权威判据：把全部基表状态灌进内存 SQLite，让 SQLite 自己执行原始 SQL。
///
/// 与本项目全部代码无关的独立实现——用 NaiveRecompute 判 NaiveRecompute
/// 是循环论证，这正是它单独存在的理由。
pub fn recompute_via_sqlite(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
) -> Result<ZSet, EngineError> {
    let conn = Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?;

    for schema in db.tables() {
        conn.execute_batch(&create_table_sql(schema))
            .map_err(|e| EngineError(e.to_string()))?;

        let base = bases.get(&schema.table).ok_or_else(|| {
            EngineError(format!("缺少基表 {} 的状态", schema.table))
        })?;

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

        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|e| EngineError(e.to_string()))?;
        for (row, weight) in base.iter() {
            if *weight < 0 {
                return Err(EngineError(format!(
                    "基表 {} 的状态含负权重 {weight}，行 {row:?}",
                    schema.table
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

    // 视图 SQL 目前仍按单表渲染；join 查询的渲染在引擎计划的 Phase 3 加入。
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database 为空".into()))?;
    let sql = view_query_to_sql(query, anchor);

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

把 oracle 原有单测改为传 `Database::single(...)` 与单元素 `BTreeMap`，并新增两条：

```rust
#[test]
fn builds_every_table_in_the_database() {
    // 两张表，只查其中一张；另一张必须也被建出来且被灌数据，
    // 否则 join 查询（Phase 3）会在 oracle 侧静默少一张表。
    let db = Database::new(vec![orders(), customers()]);
    let bases = BTreeMap::from([
        ("orders".to_string(), ZSet::from_rows([(row(Value::Text("a".into()), 10), 1)])),
        ("customers".to_string(), ZSet::from_rows([(cust_row("a"), 1)])),
    ]);
    let q = count_by_region();
    let got = recompute_via_sqlite(&db, &q, &bases).unwrap();
    assert_eq!(got.len(), 1);
}

#[test]
fn missing_base_state_for_a_declared_table_is_an_error() {
    let db = Database::new(vec![orders(), customers()]);
    let bases = BTreeMap::from([("orders".to_string(), ZSet::new())]);
    let err = recompute_via_sqlite(&db, &count_by_region(), &bases).unwrap_err();
    assert!(err.0.contains("customers"), "错误应指名缺哪张表：{}", err.0);
}
```

- [ ] **Step 5: 全绿 + 变异验证**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`

变异两条：

1. 让 oracle 只建 `db.tables()[0]` → 期望 `builds_every_table_in_the_database` 红
2. 把缺表的 `ok_or_else` 改成 `unwrap_or(&ZSet::new())` 之类的静默兜底 → 期望 `missing_base_state_for_a_declared_table_is_an_error` 红

先确认变异后能编译。还原后确认全绿。

- [ ] **Step 6: 登记门禁并提交**

在 `docs/mutation-gates.md` 的 "ivmlite-test：语义契约" 表里加两行，"已验证"栏填 `**已验证**`。

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(core): Database 类型；oracle 建立 database 中的每一张表"
```

---

## Task 4: 多表生成器

**Files:**
- Modify: `crates/ivmlite-test/src/data.rs`, `crates/ivmlite-test/src/ops.rs`

**Interfaces:**
- Consumes: `Database`（Task 3）、`Domain`、`Op`
- Produces: `gen_database(&mut StdRng, table_count: usize) -> Database`、`gen_initial(&mut StdRng, &Database, &Domain, rows_per_table: usize) -> BTreeMap<String, Vec<Row>>`、`gen_ops(&mut StdRng, &Database, &Domain, &BTreeMap<String, Vec<Row>>, count: usize) -> Vec<(String, Op)>`

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/src/ops.rs` 的测试模块新增：

```rust
#[test]
fn generated_database_tables_have_exactly_two_columns() {
    let mut rng = StdRng::seed_from_u64(1);
    let db = gen_database(&mut rng, 2);
    assert_eq!(db.len(), 2);
    for t in db.tables() {
        assert_eq!(
            t.arity(),
            2,
            "spec §9.2 第 4 条：差分 schema 固定每表 2 列——加宽到 3 列会让穷举规模涨约 8 倍，\
             这是「穷举优于随机」成立的前提，不是魔数"
        );
    }
}

#[test]
fn ops_are_tagged_with_a_table_that_exists() {
    let mut rng = StdRng::seed_from_u64(2);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 20);
    for (table, _) in gen_ops(&mut rng, &db, &domain, &initial, 200) {
        assert!(db.get(&table).is_some(), "未知表 {table}");
    }
}

#[test]
fn every_table_receives_some_ops() {
    // 若生成器只往一张表写，join 的 ΔR⋈S 与 R⋈ΔS 两条路径就只有一条被测到。
    let mut rng = StdRng::seed_from_u64(3);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 20);
    let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
    for t in db.tables() {
        let n = ops.iter().filter(|(tbl, _)| *tbl == t.table).count();
        assert!(n > 20, "表 {} 只收到 {n} 个操作，两侧 delta 路径覆盖不均", t.table);
    }
}

#[test]
fn deletes_target_rows_that_exist_in_their_own_table() {
    // 有偏采样必须按表各自维护 live 集合——用一张表的行去删另一张表是非法序列。
    let mut rng = StdRng::seed_from_u64(4);
    let db = gen_database(&mut rng, 2);
    let domain = Domain::default();
    let initial = gen_initial(&mut rng, &db, &domain, 30);
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    let mut hits = 0usize;
    let ops = gen_ops(&mut rng, &db, &domain, &initial, 300);
    for (table, op) in &ops {
        let l = live.get_mut(table).expect("表必须存在");
        match op {
            Op::Insert(r) => l.push(r.clone()),
            Op::Delete(r) => {
                let pos = l.iter().position(|x| x == r).expect("DELETE 必须命中本表存在的行");
                l.swap_remove(pos);
                hits += 1;
            }
            Op::Update { old, new } => {
                let pos = l.iter().position(|x| x == old).expect("UPDATE 必须命中本表存在的行");
                l.swap_remove(pos);
                l.push(new.clone());
                hits += 1;
            }
        }
    }
    assert!(hits > ops.len() / 10, "有偏采样产出的删改过少：{hits}/{}", ops.len());
}

#[test]
fn same_seed_yields_the_same_multi_table_sequence() {
    let make = || {
        let mut rng = StdRng::seed_from_u64(99);
        let db = gen_database(&mut rng, 2);
        let domain = Domain::default();
        let initial = gen_initial(&mut rng, &db, &domain, 10);
        gen_ops(&mut rng, &db, &domain, &initial, 50)
    };
    assert_eq!(make(), make());
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-test ops`
Expected: 编译失败，`cannot find function gen_database`

- [ ] **Step 3: 写实现**

`data.rs` 新增：

```rust
/// 生成差分用例的表结构。
///
/// 每表固定 2 列（一个可空 TEXT + 一个非空 INTEGER），列数**不是**可调参数：
/// spec §9.2 第 4 条把它定为「穷举优于随机」成立的前提，实测加宽到 3 列会让
/// 穷举规模从约 554 涨到约 4209。
pub fn gen_database(_rng: &mut StdRng, table_count: usize) -> Database {
    let tables = (0..table_count)
        .map(|i| Schema {
            table: format!("t{i}"),
            columns: vec![
                Column { name: "k".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "v".into(), ty: ColumnType::Integer, nullable: true },
            ],
        })
        .collect();
    Database::new(tables)
}

pub fn gen_initial(
    rng: &mut StdRng,
    db: &Database,
    domain: &Domain,
    rows_per_table: usize,
) -> BTreeMap<String, Vec<Row>> {
    db.tables()
        .iter()
        .map(|s| (s.table.clone(), gen_rows(rng, s, domain, rows_per_table)))
        .collect()
}
```

> 两列都可空：`v` 可空是为了让 spec §6.1 的「`SUM` 无非 NULL 输入时返回 NULL」这条路径在随机测试里真的走得到——M0 的集成测试正是为此把 `amount` 改成可空的。

`ops.rs` 改 `gen_ops`：按表各自维护 live 集合，每步先随机选表再选操作，返回 `(表名, Op)`。选表用均匀分布——`every_table_receives_some_ops` 会守着这一点。

- [ ] **Step 4: 全绿 + 变异验证**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`

变异三条：

1. `gen_database` 的列数改成 3 → 期望 `generated_database_tables_have_exactly_two_columns` 红
2. `gen_ops` 只往 `db.tables()[0]` 写 → 期望 `every_table_receives_some_ops` 红
3. `gen_ops` 的删改目标改为跨表采样（从任意表的 live 集合里取） → 期望 `deletes_target_rows_that_exist_in_their_own_table` 红

- [ ] **Step 5: 登记门禁并提交**

三条各占一行，"已验证"填 `**已验证**`。

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(test): 多表生成器——按表维护 live 集合、带表标签的操作序列"
```

---

## Task 5: 驱动多表化

**Files:**
- Modify: `crates/ivmlite-test/src/differential.rs`, `crates/ivmlite-test/src/engine.rs`, `crates/ivmlite-test/src/naive.rs`, `crates/ivmlite-test/src/buggy.rs`, `crates/ivmlite-test/src/shrink.rs`, `crates/ivmlite-test/tests/harness_catches_bugs.rs`

**Interfaces:**
- Consumes: 前四个任务的全部产物
- Produces: `TestCase { seed, database: Database, query: ViewQuery, initial: BTreeMap<String, Vec<Row>>, ops: Vec<(String, Op)>, batching }`；`gen_case(seed: u64, &Database, &Domain, rows_per_table: usize, op_count: usize, Batching) -> TestCase`（**原签名的 `&Schema` 改为 `&Database`，不新增变体**）；`Engine::create_view(&mut self, &Database, &ViewQuery, &BTreeMap<String, ZSet>) -> Result<(), EngineError>`（`apply` / `refresh` / `materialize` 签名不变）；`is_legal(&BTreeMap<String, Vec<Row>>, &[(String, Op)]) -> bool`

- [ ] **Step 1: 改 `Engine` trait 与 TestCase**

`create_view` 收 `&Database` 与按表的初始状态。`apply(table, raw)` 已经带表名（spec §8.5），**不要动**。

`run` 的每批循环改为：把该批的 `(表名, Op)` 按表分组 → 每张有变更的表各调一次 `apply` → 调一次 `refresh` → `materialize` → 不变量 → oracle 比对。参考 `bases: BTreeMap<String, ZSet>` 与引擎同步推进。

> **每批只调一次 `refresh`**，不是每张表一次。这正是 spec §8.2 所说的「N 次 apply、一次 refresh」形态，也是 consolidation 唯一能发挥作用的地方。

- [ ] **Step 2: 跟随修改参照实现与假引擎**

`NaiveRecompute` 按表持有 `base: BTreeMap<String, ZSet>` 与 `pending: Vec<(String, Row, i64)>`；`refresh` 把 pending 按表并入。`NoRetractionEngine` / `TransientDriftEngine` 跟随。

**两个假引擎必须保持原有的错误行为不变**——它们是证明框架有效的仪器，不是待修的缺陷。

- [ ] **Step 3: shrinker 的合法性门禁多表化**

`is_legal` 现在是 `fn is_legal(initial: &[Row], ops: &[Op]) -> bool`，单表假设写死在签名里。改为按表判定：

```rust
/// 序列的合法性：每个 DELETE / UPDATE 必须命中**它自己那张表**当时存在的行。
///
/// 多表化之后这条更容易出错：用 A 表的行去删 B 表是非法序列，而引擎从来
/// 没有义务处理非法输入——在非法序列上「失败」毫无意义，这正是自研 shrinker
/// 而不用 proptest 的全部理由（spec §9.3）。
pub fn is_legal(initial: &BTreeMap<String, Vec<Row>>, ops: &[(String, Op)]) -> bool {
    let mut live: BTreeMap<String, Vec<Row>> = initial.clone();
    for (table, op) in ops {
        let Some(l) = live.get_mut(table) else {
            return false; // 未知表
        };
        match op {
            Op::Insert(r) => l.push(r.clone()),
            Op::Delete(r) => match l.iter().position(|x| x == r) {
                Some(i) => {
                    l.swap_remove(i);
                }
                None => return false,
            },
            Op::Update { old, new } => match l.iter().position(|x| x == old) {
                Some(i) => {
                    l.swap_remove(i);
                    l.push(new.clone());
                }
                None => return false,
            },
        }
    }
    true
}
```

`shrink` 的三个阶段跟随：阶段一删 op 区间、阶段三删初始行（现在按表删），两者都要经 `is_legal` 门禁；阶段二缩 query 仍不需要门禁（缩 query 不影响序列合法性）。

现有 `is_legal` 单测改为多表形态，并**新增一条多表特有的**：

```rust
#[test]
fn deleting_a_row_that_exists_in_another_table_is_illegal() {
    // 单表时这个形态根本不存在；多表化后它是最容易被写错的一格。
    let initial = BTreeMap::from([
        ("t0".to_string(), vec![row(1)]),
        ("t1".to_string(), vec![]),
    ]);
    let ops = vec![("t1".to_string(), Op::Delete(row(1)))];
    assert!(!is_legal(&initial, &ops), "t1 里没有这一行，即便 t0 里有");
}
```

- [ ] **Step 4: 全绿，且验收阈值不退化**

Run: `cargo test --workspace --locked`
Expected: 全部通过。**`harness_catches_the_missing_retraction_bug` 仍须 ≥90% 检出，`failing_case_shrinks_to_under_ten_ops` 仍须 ≤10 步。** 任一退化说明重构动了不该动的东西——不许放宽断言，去查原因。

- [ ] **Step 5: 新增多表端到端测试**

```rust
#[test]
fn a_two_table_case_runs_green_against_the_reference_engine() {
    // 本 Phase 的交付判据：框架能表达多表用例。
    // 查询仍是单表聚合（join 在引擎计划的 Phase 3），但两张表都在接收变更，
    // 所以 apply 的表名路由、按表的 live 集合、oracle 的多表建立都被真正走到。
    let mut rng = StdRng::seed_from_u64(7);
    let db = gen_database(&mut rng, 2);
    let case = gen_case(7, &db, &Domain::default(), 25, 150, Batching::Chunks(5));
    let mut engine = NaiveRecompute::new();
    run(&mut engine, &case).unwrap_or_else(|f| panic!("参照实现不应失败: {f}"));
}
```

- [ ] **Step 6: 变异验证路由正确**

1. 让 `run` 把所有 delta 都投给 `db.tables()[0]` → 期望 `a_two_table_case_runs_green_against_the_reference_engine` 红（第二张表的变更丢失，oracle 比对不上）
2. 让 `run` 每张表调一次 `refresh` 而非每批一次 → **期望仍绿**（对 `NaiveRecompute` 语义等价）。这一条记录为"已知不被守护"，留给引擎计划——真正能区分的是一个把 consolidation 做在 `refresh` 里的引擎。

- [ ] **Step 7: 登记门禁并提交**

```bash
git add crates/ docs/mutation-gates.md
git commit -m "feat(test): 差分驱动多表化——按表路由 apply，每批一次 refresh"
```

---

## Self-Review

**1. Spec 覆盖**

| Spec 要求 | 落点 |
|---|---|
| §4.2 core 不依赖 rusqlite | Task 2（迁进去的是纯数据类型，SQL 渲染留在 test） |
| §5.1 权重不变量 | 未改动，由 M0 既有测试守护；Task 1 验证其有效性 |
| §8.5 `apply` 带表名 | Task 5 让它第一次真的路由到多张表——M0 只有一张表时这个参数形同虚设 |
| §8.5 refresh 与 apply 分离 | Task 5 每批一次 refresh |
| §9.2 第 4 条 每表 2 列 | Task 4（`gen_database` + 断言 + 变异） |
| §9.4 顺序确定、seed 可重放 | Task 3（`Database` 用 `Vec` 保序）、Task 4（同 seed 同序列） |
| §9.1 oracle 独立性 | Task 3 保持不变，仅扩展为多表 |

**已知缺口（有意为之）**：join 查询的 SQL 渲染、Join 算子、consolidation 均不在本计划——它们属引擎计划。本计划的 oracle 对多表 database 只渲染第一张表的单表查询，Task 3 的代码注释写明了这一点。

**2. Placeholder 扫描**：无 TBD / TODO。Task 4 Step 3 的 `gen_ops` 只给了改法描述而非完整代码——这是本计划**唯一**的例外，因为它是在现有函数上按表分解，完整重写反而会掩盖"哪里变了"。若实现者认为描述不足以照做，应报 NEEDS_CONTEXT 而非自行发挥。

**3. 类型一致性**：`BTreeMap<String, ZSet>` 作为按表状态的表示贯穿 Task 3 / 5；`BTreeMap<String, Vec<Row>>` 作为按表初始行贯穿 Task 4 / 5；`Vec<(String, Op)>` 作为带表标签的操作序列贯穿 Task 4 / 5。`Database` 内部用 `Vec<Schema>` 而非 map，因为需要保序。

---

## 执行顺序

Task 1 → 5 严格依赖。Task 1 是安全网检查，**必须先做**：后面四个任务全是在重构 M0 的代码，而重构的保障就是那些测试；先确认它们真的接得住。

本计划完成后写引擎计划（plan IR 的算子表示、`Arrangement`、Filter/Project/Aggregate、consolidation、Join），其代码将写在本计划产出的真实类型上。
