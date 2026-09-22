# M1a Phase 2：单表增量引擎 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `ivmlite-core` 里建出 v0 的增量引擎——plan IR、`Arrangement`、Filter / Project / Aggregate、delta consolidation——并让它接进 Phase 1 的差分框架，在全部枚举查询 × 有偏更新序列上跑绿。

**Architecture:** `ViewQuery`（harness 的扁平查询表示）经 `lower()` 降到 spec §5.2 的 `Plan` 树；`Plan` 再建成带状态的 `Node` 算子树。算子按 §6.1 的三类分工：Filter / Project 无状态、delta 直接穿过；Aggregate 持组状态并按 §6.2 发 retraction 对。`refresh` 先把本批 raw Δ 按 Z-set 合并，再推进算子树——consolidation 因此是引擎的**可观测**行为而非优化细节。引擎实现在 core，`ivmlite-test` 侧加一个 `impl Engine` 适配器接进 harness。

**Tech Stack:** Rust 1.95、纯 Rust 无 `unsafe`、无新依赖（`ivmlite-core` 按 §4.2 不得依赖 `rusqlite`）

**Spec:** `docs/superpowers/specs/2026-09-18-ivmlite-design.md`

**前序计划:** `docs/superpowers/plans/2026-09-20-m1a-phase1-multi-table-harness.md`（已完成并合入 master，41ea247）

---

## 本计划的范围边界与三条设计裁定

### 范围：本计划**不含 Join**

spec §11 给 M1a 设了一个内部检查点：「先完成多表框架重构并让单表引擎跑绿，再上 join。这样 join 出 bug 时能二分定位（是 join 引入的，还是框架重构就错了），而不必同时调两类 bug。」

Phase 1 已交付多表框架。本计划交付**单表引擎跑绿**，即检查点本身。Join 是 Phase 3，单独一份计划。这条分割不是保守，是那个二分论证的直接落地：join 是唯一的双线性算子，它的 `ΔR⋈ΔS` 项和两侧 arrangement 都是本计划任何部分都不具备的新失败形态。

**本计划不含的另外两项**（留给后续里程碑，此处列出以免被当成遗漏）：
- benchmark 对比（§11 的 M1 完成判定之一）。M0 的 benchmark 两个对照组都跑在 SQLite 里，而 M1a 引擎是纯内存 Rust，直接比是苹果对橘子。等 M1b 的 `ivmlite-sqlite` 把引擎接上真实 shadow table 之后才有可比性。
- `ivmlite-sql`（`sqlparser-rs` → IR）。M1b。

### 裁定一：`ViewQuery` 保留为 harness 表面，`Plan` 是引擎 IR，`lower()` 连接两者

`ViewQuery { group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate }` **就是** v0 的合法形状的扁平编码——spec §5.2 规定根算子必须是带非空 `GROUP BY` 的 `Aggregate`，于是每个合法 v0 查询都恰好是 `Scan → Filter? → Project → Aggregate`。

不把 harness 改成直接枚举 `Plan` 树，理由有三：

1. **穷举优于随机这条前提依赖扁平形状。** `enumerate` 的规模可控（单表 27 个查询）正因为空间是 `group_by × aggs × predicate` 的乘积；枚举树形 IR 要先解决「哪些树是合法的」，而那正是 `ViewQuery` 已经用类型编码掉的东西。
2. **oracle 按 `ViewQuery` 渲染 SQL。** 改成 `Plan` 要重写 `view_query_to_sql`，而 oracle 的独立性（§9.1）是整套验证的地基，不该为引擎的内部表示动它。
3. **§5.3 明说 IR 可以自由演进**——持久化的是 SQL 原文而非序列化 IR，正是为了让 IR 不承担兼容负担。多一层表示的成本因此只有 `lower()` 一个函数。

**代价与归属**：join 落地时 `ViewQuery` 是单表的，必须扩展或被取代。**这个决定归 Phase 3**，不在本计划里预判——那时算子树是真实代码而非推测，约束会自己显形。

### 裁定二：`lower()` 在边界上做 §5.2 校验，提前关闭一条已登记的缺口

`docs/mutation-gates.md` 有一行登记着（m6，最终评审发现）：§5.2 的根算子约束**只在生成器侧**成立——`ViewQuery` 可以在 `enumerate` 之外自由构造出 `group_by: vec![]`，`check_invariants` 一路放行。文末「Join 落地」清单把补边界校验排在 join 那时。

**提前到本计划**：本计划正是 `ViewQuery` 第一次跨越到 `enumerate` 之外的入口——引擎直接消费它。`lower()` 返回 `Result`，对空 `group_by` 或空 `aggs` 硬报错。那一行的「不适用」在本计划 Task 1 改成「已验证」，并从 join 清单里划掉。

### 裁定三：不引入只有一个变体的 `Expr`

spec §5.2 的 `Plan` 写作 `Filter { predicate: Expr }`、`Project { exprs: Vec<Expr> }`、`Aggregate { group_by: Vec<Expr> }`。但 §5.2 同时规定 v0 的 group-by key **只能是裸列**，§6.1 的比较运算符白名单也没有任何需要表达式树的形式。

于是 v0 的 `Expr` 会是一个只有 `Column(usize)` 一个变体的枚举——纯粹的空壳泛化。本计划用 `Vec<usize>`（投影与 group-by 列）与既有的 `Predicate`（过滤）代替。当第一个真正需要表达式的特性出现时（M4 的 `MIN`/`MAX` 之外的计算列），那时引入 `Expr` 是一次局部改动，而现在引入它是五个算子都要穿过的一层无内容的间接。

---

## Global Constraints

以下每条都来自 spec，是全部任务的隐含要求：

- **§4.2 `ivmlite-core` 不得依赖 `rusqlite`**，也不得依赖 `ivmlite-test`（后者依赖前者，反向会成环）。本计划不引入任何新依赖。
- **§4.2 全项目唯一允许 `unsafe` 的地方是 `ivmlite-sqlite`（M1b）。** 本计划一行 `unsafe` 都不应出现。
- **§5.1 权重归零的行必须删除**，不留僵尸条目。`ZSet::update` 已如此，算子状态也必须如此。
- **§5.2 根算子必须是 `Aggregate` 且 `group_by` 非空**；禁止全局聚合。
- **§6.1 `SUM` 在非 NULL 输入为零行时返回 `NULL` 而非 `0`。** 聚合状态必须同时维护累加值与非 NULL 输入计数。
- **§6.1 谓词是三值逻辑**：NULL 求值为 UNKNOWN，该行不进入结果；**不得据此认为 `NOT p` 等价于 `!p`**。
- **§6.1 整数溢出未定义**，值域由生成器夹住（`|组内和| < 2^62`）；引擎不做静态检查。
- **§6.2 聚合必须发 retraction 对**：SUM 从 100 变 150 时发 `(key,100) w=-1` 与 `(key,150) w=+1`，不是单独一行 `+1`。这是 IVM 最大的 bug 来源。
- **§6.3 `Arrangement::get` 返回迭代器而非 `Option`**（key → 多值），且 trait 必须 object-safe（用 `Box<dyn Iterator>`，不用 RPITIT）。
- **§8.5 `apply` 收未合并的原始 Δ**；`refresh` 与 `apply` 分离；`apply` 带表名。这三条签名一律不得改动。
- **§9.4 迭代顺序确定**：任何可能影响输出的迭代都用 `Vec` / `BTreeMap`，不得用 `HashMap` / `HashSet`。
- **变异门禁**：每条 spec 强制的行为都要在 `docs/mutation-gates.md` 登记一行，且「已验证」必须是真跑过变异的。改坏 → **确认仍能编译** → 跑测试 → 确认指名的测试变红 → 还原。编译失败不产生测试输出，粗心过滤时与「测试通过」一模一样。
- **测试命令必须是 `cargo test --workspace --locked --no-fail-fast`。** 省掉 `--no-fail-fast` 时上游 crate 的失败会掩盖真正的目标。
- 每次提交前 `cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets --locked -- -D warnings` 必须通过。
- 加门禁行后跑 `python3 scripts/count-mutation-gates.py --fix`，再裸跑一次确认打印「一致」。注意该脚本**只核对统计句与单元格字面内容**，不判断某行的「已验证」是否真跑过变异——「一致」不等于这张表诚实。
- 暂存具体文件，不用 `git add -A`；不得使用 `--no-verify`。提交信息结尾附：
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`

---

## File Structure

| 文件 | 职责 |
|---|---|
| `crates/ivmlite-core/src/plan.rs`（新建） | spec §5.2 的 `Plan` 枚举；`lower(&ViewQuery, &str) -> Result<Plan, PlanError>`；§5.2 的边界校验 |
| `crates/ivmlite-core/src/arrangement.rs`（新建） | spec §6.3 的 `Arrangement` trait；`MemArrangement`（M1a 的内存实现，M1b 换成 shadow table） |
| `crates/ivmlite-core/src/node.rs`（新建） | 带状态的算子树 `Node`：从 `Plan` 建树，`delta()` 按 §6.1 的三类规则推进 |
| `crates/ivmlite-core/src/agg.rs`（新建） | `Aggregate` 的组状态与 §6.2 的 retraction 逻辑。单独成文件是因为它是 v0 全部难度所在（§6.1：「v0 的全部难度集中在聚合」） |
| `crates/ivmlite-core/src/engine.rs`（新建） | `IncrementalEngine`：持有算子树与按表的 pending raw Δ；`refresh` 先 consolidate 再推进 |
| `crates/ivmlite-core/src/lib.rs`（修改） | 导出上述模块 |
| `crates/ivmlite-test/src/incremental.rs`（新建） | `impl Engine for IncrementalEngine` 适配器（本地 trait + 外部类型，允许） |
| `crates/ivmlite-test/src/lib.rs`（修改） | 导出适配器 |
| `crates/ivmlite-test/tests/harness_catches_bugs.rs`（修改） | 真实引擎接入差分框架的端到端测试 |
| `docs/mutation-gates.md`（修改） | 每个任务登记自己的门禁行 |

算子拆成 `node.rs` + `agg.rs` 两个文件而非一个：线性算子的 delta 规则是三行（`Δ(f(R)) = f(ΔR)`），聚合是几十行带状态机。放一起时后者会淹没前者，而两者的正确性论证完全不同。

---

## Task 1：Plan IR、lowering、§5.2 边界校验

**Files:**
- Create: `crates/ivmlite-core/src/plan.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ivmlite_core::{ViewQuery, Agg, AggFn, Predicate}`（已存在于 `query.rs`）
- Produces:
  - `pub enum Plan { Scan { table: String, columns: Vec<usize> }, Filter { input: Box<Plan>, predicate: Predicate }, Project { input: Box<Plan>, columns: Vec<usize> }, Aggregate { input: Box<Plan>, group_by: Vec<usize>, aggs: Vec<Agg> } }`
  - `pub struct PlanError(pub String)`
  - `pub fn lower(query: &ViewQuery, table: &str, arity: usize) -> Result<Plan, PlanError>`

> **`Join` 变体本计划不加。** spec §5.2 把它列在 `Plan` 里并标注「M1a」，但加一个所有匹配分支都要写 `unreachable!()` 的变体，等于在每个 `match` 上留一处没有测试能到达的代码——正是本项目反复在压的那一类。Phase 3 加变体时，编译器会把所有需要处理它的地方逐个指出来，这比预留占位更可靠。

### 为什么 lowering 要发出 `Project`

朴素的降法是 `Aggregate { input: Filter { Scan } }`，根本不产生 `Project`。但 spec §11 把 `Project` 列进 M1a，于是会出现一个**没有任何测试能到达**的算子。

本计划的降法让 `Project` 真的承重：`Scan` 取全部列 → `Filter` 按原始列下标求值 → `Project` 收窄到查询真正需要的列（`group_by ∪ 各 SUM 的列`）→ `Aggregate` 按收窄后的新下标工作。

于是 `Project` 在每一个枚举出来的用例上都被执行，且下标重映射是真实逻辑。举例：`group_by=[0]`、`aggs=[COUNT(*)]`、`predicate=IntGt{column:1}` 时，过滤用到列 1 而聚合只要列 0，`Project` 把 2 列收窄成 1 列。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/plan.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Predicate, ViewQuery};

    fn q(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery { group_by, aggs, predicate }
    }

    fn count() -> Agg {
        Agg { func: AggFn::Count, column: None }
    }

    fn sum(column: usize) -> Agg {
        Agg { func: AggFn::Sum, column: Some(column) }
    }

    #[test]
    fn lowers_to_scan_filter_project_aggregate() {
        // predicate 用列 1，聚合只要列 0——Project 必须把 2 列收窄成 1 列，
        // 且 Aggregate 的 group_by 下标必须重映射到收窄后的位置。
        let plan = lower(
            &q(vec![0], vec![count()], Predicate::IntGt { column: 1, value: 3 }),
            "orders",
            2,
        )
        .expect("合法查询必须能降下来");

        let Plan::Aggregate { input, group_by, aggs } = &plan else {
            panic!("根算子必须是 Aggregate（spec §5.2）：{plan:?}");
        };
        assert_eq!(group_by, &vec![0], "收窄后 group key 落在位置 0");
        assert_eq!(aggs.len(), 1);

        let Plan::Project { input, columns } = &**input else {
            panic!("Aggregate 之下必须是 Project：{input:?}");
        };
        assert_eq!(columns, &vec![0], "只有列 0 被聚合用到");

        let Plan::Filter { input, predicate } = &**input else {
            panic!("Project 之下必须是 Filter：{input:?}");
        };
        assert_eq!(predicate, &Predicate::IntGt { column: 1, value: 3 });

        let Plan::Scan { table, columns } = &**input else {
            panic!("最底层必须是 Scan：{input:?}");
        };
        assert_eq!(table, "orders");
        assert_eq!(columns, &vec![0, 1], "Scan 取全部列——Filter 按原始下标求值");
    }

    #[test]
    fn no_filter_node_when_predicate_is_none() {
        // Predicate::None 不应产生一个恒真的 Filter 节点：多一个节点就多一处
        // 每批都要走的无谓遍历，且会让「Filter 被正确跳过」这件事不可观察。
        let plan = lower(&q(vec![0], vec![count()], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, .. } = &plan else { panic!("{plan:?}") };
        let Plan::Project { input, .. } = &**input else { panic!("{input:?}") };
        assert!(
            matches!(&**input, Plan::Scan { .. }),
            "Predicate::None 之下应直接是 Scan，不得插入恒真 Filter：{input:?}"
        );
    }

    #[test]
    fn projection_keeps_group_keys_and_summed_columns_in_a_stable_order() {
        // group key 是列 1，SUM 的是列 0——收窄后的顺序必须确定且可预测，
        // 否则 Aggregate 的下标重映射无从对齐（spec §9.4）。
        let plan = lower(&q(vec![1], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, group_by, aggs } = &plan else { panic!("{plan:?}") };
        let Plan::Project { columns, .. } = &**input else { panic!("{input:?}") };
        assert_eq!(columns, &vec![1, 0], "先 group key（按原序），再各 agg 的列（按原序）");
        assert_eq!(group_by, &vec![0], "group key 重映射到收窄后的位置 0");
        assert_eq!(aggs[0].column, Some(1), "SUM 的列重映射到收窄后的位置 1");
    }

    #[test]
    fn a_column_used_as_both_group_key_and_sum_target_is_projected_once() {
        // 同一列既当 group key 又被 SUM 时不得在投影里出现两次——出现两次
        // 会让 Project 的输出行宽与 Aggregate 的预期不一致。
        let plan = lower(&q(vec![0], vec![sum(0)], Predicate::None), "orders", 2).unwrap();
        let Plan::Aggregate { input, group_by, aggs } = &plan else { panic!("{plan:?}") };
        let Plan::Project { columns, .. } = &**input else { panic!("{input:?}") };
        assert_eq!(columns, &vec![0], "去重后只投影一次");
        assert_eq!(group_by, &vec![0]);
        assert_eq!(aggs[0].column, Some(0));
    }

    #[test]
    fn empty_group_by_is_rejected_at_the_boundary() {
        // spec §5.2：禁止全局聚合——空表时它返回 1 行（值为 NULL），而分组
        // 聚合返回 0 行，「组内计数归零就删行」这条规则对前者是错的。
        // 此前这条只在生成器侧成立（enumerate 从不产出这种形状）；引擎直接
        // 消费 ViewQuery 之后，边界校验必须在这里。
        let err = lower(&q(vec![], vec![count()], Predicate::None), "orders", 2)
            .expect_err("空 group_by 必须被拒绝");
        assert!(
            err.0.contains("group_by") || err.0.contains("GROUP BY"),
            "错误信息应指名是 group_by 的问题：{}",
            err.0
        );
    }

    #[test]
    fn empty_aggs_is_rejected_at_the_boundary() {
        // 根算子必须是 Aggregate；没有任何聚合的 "Aggregate" 实际是
        // Scan→Project 直接成为视图，而那正是 §5.2 判为非法的形状
        // （Z-set 权重 2 会显示成 2 行，普通 SQL 视图显示 3 行）。
        let err = lower(&q(vec![0], vec![], Predicate::None), "orders", 2)
            .expect_err("空 aggs 必须被拒绝");
        assert!(err.0.contains("agg"), "错误信息应指名是 aggs 的问题：{}", err.0);
    }

    #[test]
    fn out_of_range_column_is_rejected() {
        // 下标越界必须在降的时候就报错，而不是等到 refresh 时 panic——
        // 引擎在 create_view 之后不应再有可预见的 panic 路径。
        let err = lower(&q(vec![7], vec![count()], Predicate::None), "orders", 2)
            .expect_err("越界 group key 必须被拒绝");
        assert!(err.0.contains('7'), "错误信息应指出越界的下标：{}", err.0);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core --locked plan`
Expected: 编译失败，`cannot find function lower` / `cannot find type Plan`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/plan.rs` 开头：

```rust
use crate::{Agg, Predicate, ViewQuery};

/// spec §5.2 的 plan IR。
///
/// v0 的合法形状恒为 `Scan → Filter? → Project → Aggregate`，由 `lower` 保证。
/// `Join` 变体留到 Phase 3 与 join 算子一起加——现在加一个所有 match 分支都
/// 只能写 `unreachable!()` 的变体，等于在每处匹配上留一段没有测试能到达的代码。
///
/// spec §5.2 把节点内的表达式写作 `Expr`，本实现用 `Vec<usize>`（列下标）与
/// `Predicate` 代替：v0 的 group-by key 只能是裸列（§5.2），谓词白名单（§6.1）
/// 也没有任何需要表达式树的形式，于是 `Expr` 在 v0 会是只有 `Column(usize)`
/// 一个变体的空壳。第一个真需要表达式的特性出现时再引入它是局部改动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Scan {
        table: String,
        columns: Vec<usize>,
    },
    Filter {
        input: Box<Plan>,
        predicate: Predicate,
    },
    Project {
        input: Box<Plan>,
        /// 要保留的**输入**列下标，按输出顺序排列。
        columns: Vec<usize>,
    },
    Aggregate {
        input: Box<Plan>,
        /// 下标相对于 `Project` 的**输出**，不是基表。
        group_by: Vec<usize>,
        /// 各 `Agg::column` 同样已重映射到 `Project` 的输出位置。
        aggs: Vec<Agg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PlanError {}

/// 把 harness 的扁平 `ViewQuery` 降成算子树，并在边界上执行 §5.2 的合法性校验。
///
/// 这是 `ViewQuery` 第一次跨越到 `enumerate` 之外的入口——引擎直接消费它。
/// `enumerate` 从不产出非法形状（`enumerate_covers_the_v0_space_and_is_nonempty`
/// 守着这一点），但 `ViewQuery` 本身可以自由构造，所以校验必须在这里。
///
/// `arity` 是基表的列数，用于下标越界检查。
pub fn lower(query: &ViewQuery, table: &str, arity: usize) -> Result<Plan, PlanError> {
    if query.group_by.is_empty() {
        return Err(PlanError(
            "spec §5.2：视图的根算子必须是带非空 GROUP BY 的 Aggregate；\
             禁止全局聚合（空表时它返回 1 行而分组聚合返回 0 行，\
             「组内计数归零就删行」这条规则对前者是错的）"
                .into(),
        ));
    }
    if query.aggs.is_empty() {
        return Err(PlanError(
            "spec §5.2：没有任何 agg 的查询实际是 Scan→Project 直接成为视图，\
             而 Z-set 权重与 SQL 行数在该形状下语义不一致"
                .into(),
        ));
    }

    let mut check = |c: usize, what: &str| -> Result<(), PlanError> {
        if c >= arity {
            Err(PlanError(format!(
                "{what} 引用了列下标 {c}，但表 {table} 只有 {arity} 列"
            )))
        } else {
            Ok(())
        }
    };
    for &c in &query.group_by {
        check(c, "group_by")?;
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            check(c, "agg")?;
        }
    }
    match &query.predicate {
        Predicate::None => {}
        Predicate::IntGt { column, .. } | Predicate::IsNotNull { column } => {
            check(*column, "predicate")?
        }
    }

    // 投影保留的列：先 group key（按原序），再各 agg 的列（按原序），去重。
    // 顺序必须确定，否则 Aggregate 的下标重映射无从对齐（spec §9.4）。
    let mut keep: Vec<usize> = Vec::new();
    for &c in &query.group_by {
        if !keep.contains(&c) {
            keep.push(c);
        }
    }
    for agg in &query.aggs {
        if let Some(c) = agg.column {
            if !keep.contains(&c) {
                keep.push(c);
            }
        }
    }
    let remap = |c: usize| {
        keep.iter()
            .position(|&k| k == c)
            .expect("keep 由 group_by 与 agg 列构造，必然包含它们")
    };

    // Scan 取全部列：Filter 的谓词按**基表**下标求值，收窄发生在 Filter 之后。
    let mut node = Plan::Scan {
        table: table.to_string(),
        columns: (0..arity).collect(),
    };
    if query.predicate != Predicate::None {
        node = Plan::Filter {
            input: Box::new(node),
            predicate: query.predicate.clone(),
        };
    }
    node = Plan::Project {
        input: Box::new(node),
        columns: keep.clone(),
    };
    Ok(Plan::Aggregate {
        input: Box::new(node),
        group_by: query.group_by.iter().map(|&c| remap(c)).collect(),
        aggs: query
            .aggs
            .iter()
            .map(|a| Agg {
                func: a.func,
                column: a.column.map(remap),
            })
            .collect(),
    })
}
```

`crates/ivmlite-core/src/lib.rs` 增加：

```rust
pub mod plan;
pub use plan::{lower, Plan, PlanError};
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过

- [ ] **Step 5: 变异验证**

逐条跑，每条都先确认**能编译**再看测试输出：

1. 删掉 `query.group_by.is_empty()` 那个分支 → 期望 `empty_group_by_is_rejected_at_the_boundary` 红
2. 删掉 `query.aggs.is_empty()` 那个分支 → 期望 `empty_aggs_is_rejected_at_the_boundary` 红
3. 删掉全部 `check(...)` 调用 → 期望 `out_of_range_column_is_rejected` 红
4. `keep` 的去重条件 `if !keep.contains(&c)` 全部去掉（直接 push）→ 期望 `a_column_used_as_both_group_key_and_sum_target_is_projected_once` 红
5. `Scan` 的 `columns` 改成 `keep.clone()`（即在 Scan 处就收窄）→ 期望 `lowers_to_scan_filter_project_aggregate` 红（Filter 的谓词下标会指向错误的列）
6. `Predicate::None` 时也插入 `Filter` 节点 → 期望 `no_filter_node_when_predicate_is_none` 红

- [ ] **Step 6: 登记门禁并提交**

`docs/mutation-gates.md` 的「ivmlite-core」表加上述 6 条，「已验证」填 `**已验证**`。

**同时**把 m6 那一行——`§5.2 根算子必须是聚合、GROUP BY 非空——ViewQuery 移进 ivmlite-core 正是为了让 M1 引擎直接消费它，边界上却没有任何校验`——从「不适用」改成 `**已验证**`，变异填「删掉 `lower` 里的 `group_by.is_empty()` 校验」，会红的测试填 `empty_group_by_is_rejected_at_the_boundary`。

**并且**把文末「Join 落地」清单里关于 §5.2 的那段删掉，因为它已经不再欠着了。清单只应列真正还欠的东西——留一条已还的账在上面，会让下一个执行清单的人对整张清单打折扣。

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/plan.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): plan IR 与 lowering，并在边界上执行 §5.2 校验"
```

---

## Task 2：`Arrangement` trait 与内存实现

**Files:**
- Create: `crates/ivmlite-core/src/arrangement.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `ivmlite_core::Row`
- Produces:
  - `pub trait Arrangement { fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>; fn update(&mut self, key: &Row, val: &Row, weight_delta: i64); fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>; }`
  - `pub struct MemArrangement`（`Default` + `new()`）

> `get` 返回迭代器而非 `Option` 是 spec §6.3 的硬要求：v0 的 group-by 每个 key 只存一个值，用不上多值，但 **join 的每一侧都是 key → 多行**。M0 已按 §6.3 为此付过账，本任务是兑现。同理，trait 必须 object-safe——用 `Box<dyn Iterator>` 而非 `-> impl Iterator`，否则 `dyn Arrangement` 不成立，类型参数会在整个算子树上传播（§6.3 实现注记）。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/arrangement.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn r(vals: Vec<i64>) -> Row {
        Row::new(vals.into_iter().map(Value::Int).collect())
    }

    #[test]
    fn one_key_can_hold_multiple_values() {
        // spec §6.3：get 返回迭代器而非 Option，因为 join 的每一侧都是
        // key → 多行。v0 的 group-by 用不上，但这个形状现在就必须成立，
        // 否则 Phase 3 要改的是整个算子树的签名。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![1]), &r(vec![20]), 3);
        let mut got: Vec<(Row, i64)> = a.get(&r(vec![1])).collect();
        got.sort();
        assert_eq!(got, vec![(r(vec![10]), 1), (r(vec![20]), 3)]);
    }

    #[test]
    fn weights_accumulate_and_zero_removes_the_entry() {
        // spec §5.1：权重归零的行必须删除，不留僵尸条目。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 2);
        a.update(&r(vec![1]), &r(vec![10]), -2);
        assert_eq!(a.get(&r(vec![1])).count(), 0, "归零后不得留下条目");
        assert_eq!(a.scan().count(), 0, "scan 也不得看到它");
    }

    #[test]
    fn a_key_with_no_values_left_disappears_from_scan() {
        // 仅删掉 (key,val) 还不够——key 本身也不能留成空壳，
        // 否则 scan 的行数会随历史增长而不随当前状态。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), 1);
        a.update(&r(vec![2]), &r(vec![20]), 1);
        a.update(&r(vec![1]), &r(vec![10]), -1);
        let keys: Vec<Row> = a.scan().map(|(k, _, _)| k).collect();
        assert_eq!(keys, vec![r(vec![2])], "空掉的 key 必须消失");
    }

    #[test]
    fn negative_weights_are_representable() {
        // 中间 delta 里负权重合法（spec §5.1）——只有最终物化结果里不该有。
        let mut a = MemArrangement::new();
        a.update(&r(vec![1]), &r(vec![10]), -5);
        assert_eq!(a.get(&r(vec![1])).collect::<Vec<_>>(), vec![(r(vec![10]), -5)]);
    }

    #[test]
    fn scan_order_is_deterministic() {
        // spec §9.4：失败用例要凭 seed 精确重放，任何可能影响输出的迭代
        // 都必须来自有序容器。
        let build = || {
            let mut a = MemArrangement::new();
            for k in [3, 1, 2] {
                for v in [30, 10, 20] {
                    a.update(&r(vec![k]), &r(vec![v]), 1);
                }
            }
            a.scan().collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
        let keys: Vec<Row> = build().into_iter().map(|(k, _, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "scan 必须按 key 有序");
    }

    #[test]
    fn get_on_a_missing_key_is_empty_not_a_panic() {
        let a = MemArrangement::new();
        assert_eq!(a.get(&r(vec![99])).count(), 0);
    }

    #[test]
    fn mem_arrangement_is_usable_as_a_trait_object() {
        // spec §6.3 实现注记：trait 必须 object-safe，否则算子树上会被迫
        // 传播类型参数。这个测试就是那条约束的编译期门禁。
        let mut a: Box<dyn Arrangement> = Box::new(MemArrangement::new());
        a.update(&r(vec![1]), &r(vec![10]), 1);
        assert_eq!(a.get(&r(vec![1])).count(), 1);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core --locked arrangement`
Expected: 编译失败，`cannot find type MemArrangement`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/arrangement.rs` 开头：

```rust
use std::collections::BTreeMap;

use crate::Row;

/// spec §6.3。key → 多个 (value, weight)。
///
/// `get` 返回迭代器而非 `Option`：v0 的 group-by 每个 key 只存一个值，用不上
/// 多值，但 join 的每一侧都是 key → 多行（§6.3 明写这是 M0 就不许做出会让
/// join 返工的决定的主要落点）。
///
/// 用 `Box<dyn Iterator>` 而非 RPITIT 是为了 object-safety：算子需要持有
/// `dyn Arrangement`（M1b 的实现来自 `ivmlite-sqlite`），RPITIT 会让该 trait
/// 不是 object-safe，迫使类型参数在整个算子树上传播。
pub trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}

/// M1a 的内存实现。M1b 会另加一个走 SQLite shadow table 的实现。
///
/// 两层都用 `BTreeMap`：spec §9.4 要求任何可能影响输出的迭代顺序都确定，
/// 而 `scan()` 的顺序直接进 delta 流。
#[derive(Debug, Clone, Default)]
pub struct MemArrangement {
    inner: BTreeMap<Row, BTreeMap<Row, i64>>,
}

impl MemArrangement {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Arrangement for MemArrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_> {
        match self.inner.get(key) {
            Some(vals) => Box::new(vals.iter().map(|(v, &w)| (v.clone(), w))),
            None => Box::new(std::iter::empty()),
        }
    }

    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64) {
        if weight_delta == 0 {
            return;
        }
        let vals = self.inner.entry(key.clone()).or_default();
        let w = vals.entry(val.clone()).or_insert(0);
        *w += weight_delta;
        // spec §5.1：归零即删，不留僵尸条目。key 空掉后连 key 一起删，
        // 否则 scan 的规模会随历史而非当前状态增长。
        if *w == 0 {
            vals.remove(val);
            if vals.is_empty() {
                self.inner.remove(key);
            }
        }
    }

    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_> {
        Box::new(
            self.inner
                .iter()
                .flat_map(|(k, vals)| vals.iter().map(move |(v, &w)| (k.clone(), v.clone(), w))),
        )
    }
}
```

`crates/ivmlite-core/src/lib.rs` 增加：

```rust
pub mod arrangement;
pub use arrangement::{Arrangement, MemArrangement};
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过

- [ ] **Step 5: 变异验证**

1. `get` 改成最多返回一个值（`vals.iter().take(1)`）→ 期望 `one_key_can_hold_multiple_values` 红
2. 删掉 `if *w == 0 { vals.remove(val); ... }` 整段 → 期望 `weights_accumulate_and_zero_removes_the_entry` 红
3. 只删 `if vals.is_empty() { self.inner.remove(key); }` 一行 → 期望 `a_key_with_no_values_left_disappears_from_scan` 红
4. 外层 `BTreeMap` 换 `HashMap`（`scan` 的顺序随之不定）→ 期望 `scan_order_is_deterministic` 红。**注意这一条是统计性的**：`HashMap` 的 `RandomState` 逐进程重新播种，3 个 key 有 1/3! ≈ 16.7% 概率凑巧有序。跑至少 10 次进程确认，并在门禁行里如实写明它是统计性守护而非绝对守护。
5. `update` 里 `if weight_delta == 0 { return; }` 删掉 → 期望**仍绿**（`or_insert(0)` 之后加 0 再判零会把条目删掉，行为等价）。这一条记为「已知不被守护」，理由是它纯属短路优化，没有可观察的语义。

- [ ] **Step 6: 登记门禁并提交**

前 4 条填 `**已验证**`（第 4 条注明统计性），第 5 条填 `不适用` 并写明理由。

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/arrangement.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): Arrangement trait 与内存实现"
```

---

## Task 3：线性算子——Filter 与 Project

**Files:**
- Create: `crates/ivmlite-core/src/node.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `Plan`（Task 1）、`ZSet`、`Row`、`Value`、`Predicate`
- Produces:
  - `pub enum Node { Scan { table: String }, Filter { input: Box<Node>, predicate: Predicate }, Project { input: Box<Node>, columns: Vec<usize> }, Aggregate { input: Box<Node>, agg: AggState } }`（`Aggregate` 变体的内部类型由 Task 4 定义；**本任务先只实现前三个变体，`Node::build` 遇到 `Plan::Aggregate` 时返回 `NodeError`**）
  - `pub struct NodeError(pub String)`
  - `impl Node { pub fn build(plan: &Plan) -> Result<Node, NodeError>; pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet }`

> **为什么 `Aggregate` 分两个任务**：线性算子的 delta 规则是 `Δ(f(R)) = f(ΔR)`——三行，无状态，正确性一眼可证。聚合是带状态机的几十行，且 §6.1 明写「v0 的全部难度集中在聚合」。合成一个任务时，评审必须同时评两类完全不同的正确性论证，而其中一类会淹没另一类。

### `Scan` 的 delta 规则与表名

`Scan { table }` 的 `delta(t, input)` 在 `t == table` 时返回 `input`，否则返回空 `ZSet`。单表时这个判断看似多余，但它正是 join 的两侧各自只吸收自己那张表的 delta 的机制——Phase 3 不必改动 `Scan`。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/node.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Predicate, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    #[test]
    fn scan_only_absorbs_its_own_table() {
        // 单表时这看似多余，但它正是 join 两侧各自只吸收自己那张表的
        // delta 的机制（Phase 3 不必改动 Scan）。
        let mut n = Node::build(&Plan::Scan {
            table: "orders".into(),
            columns: vec![0, 1],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2)]), 1)]);
        assert_eq!(n.delta("orders", &d), d, "自己的表：原样穿过");
        assert_eq!(n.delta("customers", &d), ZSet::new(), "别人的表：空");
    }

    #[test]
    fn filter_passes_deltas_through_unchanged_for_matching_rows() {
        // spec §6.1：线性算子 Δ(f(R)) = f(ΔR)——delta 直接穿过，无状态。
        // 权重必须原样保留，包括负权重（撤回一行满足谓词的行，
        // 撤回动作本身也要穿过去）。
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(5)]), 1), (row(vec![int(9)]), -2)]);
        assert_eq!(n.delta("t", &d), d);
    }

    #[test]
    fn filter_drops_rows_that_fail_the_predicate() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1)]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn filter_treats_null_as_unknown_not_as_false_negation() {
        // spec §6.1 三值逻辑：v 取 {1, NULL, 5} 时 `v > 3` 命中 1 行，
        // `NOT (v > 3)` 也只命中 1 行——两者加起来是 2 而不是 3。
        // 这个测试钉的是 NULL 行两边都不进，而不是「NULL 等价于 false」。
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IntGt { column: 0, value: 3 },
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(1)]), 1),
            (row(vec![Value::Null]), 1),
            (row(vec![int(5)]), 1),
        ]);
        let got = n.delta("t", &d);
        assert_eq!(got, ZSet::from_rows([(row(vec![int(5)]), 1)]));
        assert_eq!(got.weight_of(&row(vec![Value::Null])), 0, "NULL 行不得进入结果");
    }

    #[test]
    fn is_not_null_predicate_filters_null_rows() {
        let mut n = Node::build(&Plan::Filter {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            predicate: Predicate::IsNotNull { column: 0 },
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![Value::Null]), 1), (row(vec![int(5)]), 1)]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(5)]), 1)]));
    }

    #[test]
    fn project_narrows_columns_and_preserves_weights() {
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1, 2] }),
            columns: vec![2, 0],
        })
        .unwrap();
        let d = ZSet::from_rows([(row(vec![int(1), int(2), int(3)]), 4)]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![int(3), int(1)]), 4)]),
            "列按 columns 给出的顺序重排，权重原样保留"
        );
    }

    #[test]
    fn project_merges_rows_that_become_identical_after_narrowing() {
        // 两行在收窄后变成同一行时，权重必须相加而不是后者覆盖前者——
        // 这是 Z-set 语义，也是 Project 唯一一处不平凡的地方。
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1] }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), 3),
        ]);
        assert_eq!(n.delta("t", &d), ZSet::from_rows([(row(vec![int(7)]), 5)]));
    }

    #[test]
    fn project_drops_rows_whose_weights_cancel_after_narrowing() {
        // 收窄后权重相消为 0 的行必须消失（spec §5.1），不得留成权重 0 的条目。
        let mut n = Node::build(&Plan::Project {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0, 1] }),
            columns: vec![0],
        })
        .unwrap();
        let d = ZSet::from_rows([
            (row(vec![int(7), int(1)]), 2),
            (row(vec![int(7), int(2)]), -2),
        ]);
        assert!(n.delta("t", &d).is_empty(), "相消后必须为空");
    }

    #[test]
    fn building_an_aggregate_is_an_error_until_task_4() {
        // 占位：Task 4 把这个测试删掉并换成真实的聚合测试。留它在这里是为了
        // 「未实现」有一个明确的、会被执行到的形态，而不是一个 panic。
        let err = Node::build(&Plan::Aggregate {
            input: Box::new(Plan::Scan { table: "t".into(), columns: vec![0] }),
            group_by: vec![0],
            aggs: vec![],
        })
        .expect_err("Task 3 尚未实现 Aggregate");
        assert!(err.0.contains("Aggregate"));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core --locked node`
Expected: 编译失败，`cannot find type Node`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/node.rs` 开头：

```rust
use crate::{Plan, Predicate, Row, Value, ZSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeError(pub String);

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NodeError {}

/// 带状态的算子树。由 `Plan` 建出，此后 `delta` 反复被调用。
///
/// 与 `Plan` 分开是因为 `Plan` 是纯描述（可比较、可打印、将来可从 SQL 重建），
/// 而算子要持有状态。spec §5.3 存 SQL 原文而非序列化 IR，正是靠这条分离。
#[derive(Debug)]
pub enum Node {
    Scan {
        table: String,
    },
    Filter {
        input: Box<Node>,
        predicate: Predicate,
    },
    Project {
        input: Box<Node>,
        columns: Vec<usize>,
    },
}

impl Node {
    pub fn build(plan: &Plan) -> Result<Node, NodeError> {
        match plan {
            Plan::Scan { table, .. } => Ok(Node::Scan {
                table: table.clone(),
            }),
            Plan::Filter { input, predicate } => Ok(Node::Filter {
                input: Box::new(Node::build(input)?),
                predicate: predicate.clone(),
            }),
            Plan::Project { input, columns } => Ok(Node::Project {
                input: Box::new(Node::build(input)?),
                columns: columns.clone(),
            }),
            Plan::Aggregate { .. } => Err(NodeError(
                "Aggregate 算子尚未实现（本计划 Task 4）".into(),
            )),
        }
    }

    /// 把某张表的一批 delta 推过本节点，返回本节点输出的 delta。
    ///
    /// spec §6.1：线性算子满足 `Δ(f(R)) = f(ΔR)`，于是 Filter / Project 无状态，
    /// delta 直接穿过。
    pub fn delta(&mut self, table: &str, input: &ZSet) -> ZSet {
        match self {
            Node::Scan { table: own } => {
                if own == table {
                    input.clone()
                } else {
                    ZSet::new()
                }
            }
            Node::Filter { input: child, predicate } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    if passes(predicate, row) {
                        out.update(row.clone(), w);
                    }
                }
                out
            }
            Node::Project { input: child, columns } => {
                let upstream = child.delta(table, input);
                let mut out = ZSet::new();
                for (row, &w) in upstream.iter() {
                    // 收窄后可能与另一行重合——`ZSet::update` 累加权重并在
                    // 归零时删条目，正是需要的 Z-set 语义。
                    let narrowed = Row::new(columns.iter().map(|&c| row.get(c).clone()).collect());
                    out.update(narrowed, w);
                }
                out
            }
        }
    }
}

/// spec §6.1 的三值逻辑：NULL 求值为 UNKNOWN，该行不进入结果。
///
/// 返回 `bool` 而非三值枚举，是因为在**筛选**语义下「不通过」与「未知」
/// 合并为同一种处理。但**不得据此认为 `NOT p` 等价于 `!p`**——v0 的谓词
/// 白名单里没有 `NOT`，正是因为每加一个都要重新论证一次三值逻辑。
fn passes(predicate: &Predicate, row: &Row) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::IntGt { column, value } => match row.get(*column) {
            Value::Int(i) => i > value,
            // NULL > 3 是 UNKNOWN；Text > Int 在 v0 的枚举里不会出现
            // （enumerate_only_sums_integer_columns 之外，IntGt 只对 Integer 列生成）。
            _ => false,
        },
        Predicate::IsNotNull { column } => !matches!(row.get(*column), Value::Null),
    }
}
```

`crates/ivmlite-core/src/lib.rs` 增加：

```rust
pub mod node;
pub use node::{Node, NodeError};
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过

- [ ] **Step 5: 变异验证**

1. `Node::Scan` 的 `delta` 去掉表名判断（恒返回 `input.clone()`）→ 期望 `scan_only_absorbs_its_own_table` 红
2. `passes` 对 `Value::Null` 返回 `true` → 期望 `filter_treats_null_as_unknown_not_as_false_negation` 与 `is_not_null_predicate_filters_null_rows` 红
3. `Project` 的 `out.update(narrowed, w)` 改成直接插入覆盖（用一个临时 `BTreeMap` 并 `insert`）→ 期望 `project_merges_rows_that_become_identical_after_narrowing` 红
4. `Filter` 的 `out.update(row.clone(), w)` 把 `w` 换成 `1` → 期望 `filter_passes_deltas_through_unchanged_for_matching_rows` 红
5. `Project` 的 `columns.iter().map(...)` 改成原样克隆整行 → 期望 `project_narrows_columns_and_preserves_weights` 红

- [ ] **Step 6: 登记门禁并提交**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): 线性算子 Filter 与 Project"
```

---

## Task 4：Aggregate 与 retraction 语义

**Files:**
- Create: `crates/ivmlite-core/src/agg.rs`
- Modify: `crates/ivmlite-core/src/node.rs`（加 `Aggregate` 变体，删 Task 3 的占位测试）
- Modify: `crates/ivmlite-core/src/lib.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `Agg`、`AggFn`、`Row`、`Value`、`ZSet`、`Node`（Task 3）
- Produces:
  - `pub struct AggState`，含 `pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState` 与 `pub fn absorb(&mut self, input: &ZSet) -> ZSet`
  - `Node::Aggregate { input: Box<Node>, state: AggState }` 变体，`Node::build` 不再对 `Plan::Aggregate` 报错

> **这是本计划全部难度所在。** spec §6.2 开篇即写「这是 IVM 最大的 bug 来源，必须严格遵守」。

### 状态与 retraction 契约

每个 group 维护：

- `rows: i64` — 组内行的权重和。`COUNT(*)` 的输出就是它；归零时该组从输出中消失。
- 每个 agg 一个累加器：
  - `Count` 不需要额外状态（用 `rows`）。
  - `Sum` 需要**两个**量：`sum: i64`（累加值）与 `non_null: i64`（非 NULL 输入的权重和）。§6.1 明写只维护累加值的实现会在「组非空但该列全为 NULL」时输出 `0`，而 SQLite 输出 `NULL`，且这个不一致是**静默**的。
- `emitted: Option<Row>` — **本组上一次对外发出过的行**。§6.2：聚合必须记住自己发过什么才能撤回它。这是聚合需要状态的真正原因。

发射规则（每批结束时对每个被触及的 group 各做一次）：

```
new_out = if rows > 0 { Some(组的输出行) } else { None }
if new_out != emitted:
    if let Some(old) = emitted      -> out.update(old, -1)      // 撤回旧行
    if let Some(new) = &new_out     -> out.update(new.clone(), +1)  // 发出新行
    emitted = new_out
```

`new_out == emitted` 时**什么都不发**——这不是优化，是正确性：多发一对 `(-1, +1)` 会在下游产生不必要的抖动，而漏发则是 §6.2 说的那个最大 bug 源。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/agg.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggFn, Value, ZSet};

    fn row(vals: Vec<Value>) -> Row {
        Row::new(vals)
    }

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    fn sum_state() -> AggState {
        // group key 是列 0，SUM 的是列 1
        AggState::new(vec![0], vec![Agg { func: AggFn::Sum, column: Some(1) }])
    }

    fn count_state() -> AggState {
        AggState::new(vec![0], vec![Agg { func: AggFn::Count, column: None }])
    }

    #[test]
    fn a_changed_sum_emits_a_retraction_pair_not_a_bare_insert() {
        // spec §6.2：SUM 从 100 变 150 时发的是 (key,100) w=-1 与 (key,150) w=+1，
        // 不是单独一行 +1。这是 IVM 最大的 bug 来源。
        let mut s = sum_state();
        let first = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));
        assert_eq!(first, ZSet::from_rows([(row(vec![txt("a"), int(100)]), 1)]));

        let second = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(50)]), 1)]));
        assert_eq!(
            second,
            ZSet::from_rows([
                (row(vec![txt("a"), int(100)]), -1),
                (row(vec![txt("a"), int(150)]), 1),
            ]),
            "必须撤回旧输出行并发出新行"
        );
    }

    #[test]
    fn an_unchanged_group_emits_nothing() {
        // 组被触及、但它的**输出**没变时，一对 (-1,+1) 也不该发。
        //
        // 输入必须是两条**不同的**行（一进一出），不能是同一行的 +1/-1：
        // 后者在 `ZSet::from_rows` 里就相消成空集了，`absorb` 根本不会看到
        // 任何输入，于是 `touched` 为空、发射循环一次都不执行——测试会通过，
        // 但通过的理由与它声称守护的东西无关，而「把 `new_out != emitted`
        // 改成恒真」这个变异也不会让它变红。
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("a"), int(2)]), 1),
        ]));
        // 换掉组内一行：行变了，但组的行数没变，于是 COUNT 的输出不变。
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(3)]), 1),
            (row(vec![txt("a"), int(1)]), -1),
        ]));
        assert!(d.is_empty(), "组被触及但输出未变时不得发射：{d:?}");
    }

    #[test]
    fn a_group_that_empties_is_retracted_and_not_replaced() {
        // spec §5.2：分组聚合在空表时返回 0 行（与全局聚合不同）。
        // 组内计数归零时只发撤回，不发任何新行。
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(1)]), -1)]),
            "只撤回，不发新行"
        );
    }

    #[test]
    fn sum_over_only_null_inputs_is_null_not_zero() {
        // spec §6.1（已实测）：组非空但该列全为 NULL 时，组出现、COUNT(*) 为正、
        // 而 SUM 为 NULL。只维护累加值的实现会输出 0，与 SQLite 静默不一致。
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), Value::Null]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), Value::Null]), 1)]),
            "SUM 必须是 NULL 而不是 Int(0)"
        );
    }

    #[test]
    fn sum_that_genuinely_totals_zero_is_int_zero_not_null() {
        // 与上一条相对：有非 NULL 输入、其和恰为 0 时必须是 Int(0)。
        // 只看「和是否为 0」的实现会在这里输出 NULL。
        let mut s = sum_state();
        let d = s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), int(-5)]), 1),
        ]));
        assert_eq!(d, ZSet::from_rows([(row(vec![txt("a"), int(0)]), 1)]));
    }

    #[test]
    fn a_group_whose_last_non_null_input_leaves_falls_back_to_null() {
        // 非 NULL 输入被删光、但组仍非空时，SUM 必须从 Int 变回 NULL——
        // 这条路径只有同时维护 sum 与 non_null 计数才走得对。
        let mut s = sum_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(5)]), 1),
            (row(vec![txt("a"), Value::Null]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(5)]), -1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(5)]), -1),
                (row(vec![txt("a"), Value::Null]), 1),
            ]),
            "组还在（那行 NULL 仍在），但 SUM 退回 NULL"
        );
    }

    #[test]
    fn groups_are_independent() {
        let mut s = count_state();
        s.absorb(&ZSet::from_rows([
            (row(vec![txt("a"), int(1)]), 1),
            (row(vec![txt("b"), int(1)]), 1),
        ]));
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 1)]));
        assert_eq!(
            d,
            ZSet::from_rows([
                (row(vec![txt("a"), int(1)]), -1),
                (row(vec![txt("a"), int(2)]), 1),
            ]),
            "只有 a 组受影响，b 组不得出现在 delta 里"
        );
    }

    #[test]
    fn a_null_group_key_is_a_group_like_any_other() {
        // NULL 作为 group key 在 SQL GROUP BY 里自成一组（与 WHERE 的三值
        // 逻辑不同）。差分测试的值域 NULL 高频，这条路径一定会被走到。
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
        assert_eq!(d, ZSet::from_rows([(row(vec![Value::Null, int(1)]), 1)]));
    }

    #[test]
    fn emitted_output_weight_is_always_one() {
        // spec §5.2：group key → 恰好一个输出行，__w 在最终输出中恒为 1。
        // 权重只出现在内部 delta 与算子状态里。
        let mut s = count_state();
        let d = s.absorb(&ZSet::from_rows([(row(vec![txt("a"), int(1)]), 5)]));
        assert_eq!(
            d,
            ZSet::from_rows([(row(vec![txt("a"), int(5)]), 1)]),
            "输入权重 5 变成 COUNT=5 的一行，输出权重是 1 而不是 5"
        );
    }
}
```

`crates/ivmlite-core/src/node.rs` 的测试模块里，把 `building_an_aggregate_is_an_error_until_task_4` **删掉**，换成：

```rust
    #[test]
    fn aggregate_can_be_built_and_runs_through_the_tree() {
        // 端到端：Scan → Filter → Project → Aggregate 整棵树推一批 delta。
        let plan = crate::lower(
            &crate::ViewQuery {
                group_by: vec![0],
                aggs: vec![crate::Agg { func: crate::AggFn::Count, column: None }],
                predicate: Predicate::IntGt { column: 1, value: 3 },
            },
            "t",
            2,
        )
        .unwrap();
        let mut n = Node::build(&plan).unwrap();
        let d = ZSet::from_rows([
            (row(vec![Value::Text("a".into()), int(9)]), 1),
            (row(vec![Value::Text("a".into()), int(1)]), 1), // 被 Filter 挡掉
        ]);
        assert_eq!(
            n.delta("t", &d),
            ZSet::from_rows([(row(vec![Value::Text("a".into()), int(1)]), 1)]),
            "只有通过谓词的那一行进入计数"
        );
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core --locked agg`
Expected: 编译失败，`cannot find type AggState`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/agg.rs` 开头：

```rust
use std::collections::BTreeMap;

use crate::{Agg, AggFn, Row, Value, ZSet};

/// 一个 agg 的累加器。
///
/// `Sum` 必须同时维护 `sum` 与 `non_null`：spec §6.1 明写，只维护累加值的
/// 实现会在「组非空但该列全为 NULL」时输出 `0`，而 SQLite 输出 `NULL`，
/// 且这个不一致是静默的。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Acc {
    sum: i64,
    non_null: i64,
}

/// 一个 group 的状态。
#[derive(Debug, Clone, Default)]
struct Group {
    /// 组内行的权重和。`COUNT(*)` 的输出即此值；归零时该组从输出中消失。
    rows: i64,
    accs: Vec<Acc>,
    /// 本组上一次对外发出过的行。spec §6.2：聚合必须记住自己发过什么
    /// 才能撤回它——这是聚合需要状态的真正原因。
    emitted: Option<Row>,
}

/// spec §6.2 的聚合算子状态。
///
/// `BTreeMap` 而非 `HashMap`：group 的遍历顺序进入 delta 流，而 spec §9.4
/// 要求失败用例能凭 seed 精确重放。
#[derive(Debug, Clone)]
pub struct AggState {
    group_by: Vec<usize>,
    aggs: Vec<Agg>,
    groups: BTreeMap<Row, Group>,
}

impl AggState {
    pub fn new(group_by: Vec<usize>, aggs: Vec<Agg>) -> AggState {
        AggState {
            group_by,
            aggs,
            groups: BTreeMap::new(),
        }
    }

    /// 吸收一批输入 delta，返回本算子**对外**发出的 delta。
    pub fn absorb(&mut self, input: &ZSet) -> ZSet {
        // 先把本批的全部变更并进组状态，记下哪些组被触及；发射统一在之后做。
        // 分两阶段是必要的：同一个组在一批里可能被多行触及，逐行发射会发出
        // 一串中间状态的 retraction 对，而对外只应看到本批的净变化。
        let mut touched: Vec<Row> = Vec::new();
        for (row, &w) in input.iter() {
            let key = Row::new(self.group_by.iter().map(|&c| row.get(c).clone()).collect());
            if !touched.contains(&key) {
                touched.push(key.clone());
            }
            let g = self
                .groups
                .entry(key)
                .or_insert_with(|| Group {
                    rows: 0,
                    accs: vec![Acc::default(); self.aggs.len()],
                    emitted: None,
                });
            g.rows += w;
            for (i, agg) in self.aggs.iter().enumerate() {
                if agg.func != AggFn::Sum {
                    continue;
                }
                let col = agg.column.expect("SUM 必须带列（lower 已校验）");
                if let Value::Int(v) = row.get(col) {
                    g.accs[i].sum += v * w;
                    g.accs[i].non_null += w;
                }
                // NULL 输入既不进 sum 也不进 non_null——这正是「全为 NULL 时
                // 输出 NULL」那条契约在状态层面的落点。
            }
        }

        let mut out = ZSet::new();
        // 发射顺序取自 BTreeMap 的有序遍历而非 `touched` 的到达顺序，
        // 以免输出 delta 的顺序依赖输入行的排列（spec §9.4）。
        let mut keys: Vec<Row> = touched;
        keys.sort();
        for key in keys {
            let Some(g) = self.groups.get_mut(&key) else {
                continue;
            };
            let new_out = if g.rows > 0 {
                let mut vals: Vec<Value> = key.0.clone();
                for (i, agg) in self.aggs.iter().enumerate() {
                    vals.push(match agg.func {
                        AggFn::Count => Value::Int(g.rows),
                        AggFn::Sum => {
                            if g.accs[i].non_null == 0 {
                                Value::Null
                            } else {
                                Value::Int(g.accs[i].sum)
                            }
                        }
                    });
                }
                Some(Row::new(vals))
            } else {
                None
            };

            if new_out != g.emitted {
                if let Some(old) = &g.emitted {
                    out.update(old.clone(), -1);
                }
                if let Some(new) = &new_out {
                    out.update(new.clone(), 1);
                }
                g.emitted = new_out;
            }

            // spec §5.1：组彻底空掉后不留僵尸状态。
            if g.rows == 0 && g.emitted.is_none() {
                self.groups.remove(&key);
            }
        }
        out
    }
}
```

`crates/ivmlite-core/src/node.rs` 的 `Node` 枚举加变体，`build` 与 `delta` 各加一支：

```rust
    Aggregate {
        input: Box<Node>,
        state: crate::AggState,
    },
```

```rust
            Plan::Aggregate { input, group_by, aggs } => Ok(Node::Aggregate {
                input: Box::new(Node::build(input)?),
                state: crate::AggState::new(group_by.clone(), aggs.clone()),
            }),
```

```rust
            Node::Aggregate { input: child, state } => {
                let upstream = child.delta(table, input);
                state.absorb(&upstream)
            }
```

`crates/ivmlite-core/src/lib.rs` 增加：

```rust
pub mod agg;
pub use agg::AggState;
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过

- [ ] **Step 5: 变异验证**

1. 删掉 `if let Some(old) = &g.emitted { out.update(old.clone(), -1); }` → 期望 `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert` 与 `a_group_that_empties_is_retracted_and_not_replaced` 红
2. 发射条件 `if new_out != g.emitted` 改成恒真 → 期望 `an_unchanged_group_emits_nothing` 红
3. `if g.accs[i].non_null == 0 { Value::Null }` 的判据改成 `g.accs[i].sum == 0` → 期望 `sum_that_genuinely_totals_zero_is_int_zero_not_null` 红
4. 删掉 `non_null` 字段的维护（`g.accs[i].non_null += w` 那行），判据改成只看 `rows` → 期望 `sum_over_only_null_inputs_is_null_not_zero` 红
5. `g.accs[i].sum += v * w` 改成 `+= v`（忽略权重）→ 期望 `emitted_output_weight_is_always_one` 或 `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert` 红；把实际变红的那个填进门禁表
6. `out.update(new.clone(), 1)` 的权重改成 `g.rows` → 期望 `emitted_output_weight_is_always_one` 红
7. `groups` 的 `BTreeMap` 换 `HashMap`，且 `keys.sort()` 删掉 → 期望某个多组测试红。**这条是统计性的**，跑至少 10 次进程确认，门禁行里如实写明
8. 删掉 `if g.rows == 0 && g.emitted.is_none() { self.groups.remove(&key); }` → 期望**仍绿**（僵尸组状态不可观察，因为它的 `emitted` 为 `None`、`rows` 为 0，不会再发射任何东西）。记为「已知不被守护」，理由写明：它是内存回收而非语义，只有在长序列下的内存占用上可见，而差分测试不测内存

- [ ] **Step 6: 登记门禁并提交**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/agg.rs crates/ivmlite-core/src/node.rs crates/ivmlite-core/src/lib.rs docs/mutation-gates.md
git commit -m "feat(core): Aggregate 算子与 §6.2 的 retraction 语义"
```

---

## Task 5：引擎与 harness 适配器——差分框架第一次跑真实引擎

**Files:**
- Create: `crates/ivmlite-core/src/engine.rs`
- Create: `crates/ivmlite-test/src/incremental.rs`
- Modify: `crates/ivmlite-core/src/lib.rs`、`crates/ivmlite-test/src/lib.rs`
- Modify: `crates/ivmlite-test/tests/harness_catches_bugs.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Consumes: `lower`（Task 1）、`Node`（Task 3/4）、`Database`、`ViewQuery`、`ZSet`
- Produces:
  - `pub struct IncrementalEngine`，含：
    - `pub fn new() -> IncrementalEngine`
    - `pub fn create_view(&mut self, db: &Database, query: &ViewQuery, initial: &BTreeMap<String, ZSet>) -> Result<(), EngineError>`
    - `pub fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>`
    - `pub fn refresh(&mut self) -> Result<(), EngineError>`
    - `pub fn materialize(&self) -> ZSet`
  - `pub struct EngineError(pub String)`（core 自己的，与 `ivmlite-test` 的同名类型不同）
- `ivmlite-test` 侧：`impl crate::Engine for ivmlite_core::IncrementalEngine`

> 引擎放在 `ivmlite-core` 而不是 `ivmlite-test`：它是核心产物，M1b 的 `ivmlite-sqlite` 也要消费它。`Engine` trait 住在 `ivmlite-test`，而 core 不得依赖 test（§4.2，且反向会成环）。适配器写在 test 侧——本地 trait + 外部类型，孤儿规则允许。

### 本任务的交付判据

**差分测试框架第一次在一个真正的增量引擎上跑绿**：枚举出的每个查询 × 有偏更新序列 × 逐批 oracle 比对 × 批次无关性。这是 M1a 的检查点本身（spec §11）。

本任务**不做 consolidation**——`refresh` 把 pending 里的 raw Δ 逐条推进算子树。Task 6 再加合并，且那时 consolidation 的效果必须是**可观测**的。先不做是为了让 Task 6 的门禁有一个真实的「之前」可对照：如果 Task 5 就顺手合并了，Task 6 的变异会无从判断是不是真的在守 consolidation。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-test/tests/harness_catches_bugs.rs` 末尾：

```rust
/// M1a 检查点（spec §11）：差分框架第一次在真实增量引擎上跑绿。
///
/// 这条测试与 `naive_engine_is_green_across_many_seeds` 的结构相同，
/// 但被测对象换成了 `IncrementalEngine`——它是增量的，而参照实现是全量
/// 重算，两者在每个 refresh 点都要与 oracle 一致。
#[test]
fn incremental_engine_is_green_across_the_enumerated_space() {
    let db = gen_database(2);
    let domain = Domain::default();
    let queries = enumerate(&db.tables()[0]);
    let mut checked = 0usize;
    for seed in seed_range() {
        let query = &queries[seed as usize % queries.len()];
        let case = gen_case_with_query(seed, &db, &domain, query.clone(), 20, 60, Batching::Chunks(4));
        let mut engine = IncrementalEngine::new();
        if let Err(f) = run(&mut engine, &case) {
            panic!("增量引擎在 seed={seed} 上与 oracle 不一致：{f}");
        }
        checked += 1;
    }
    assert!(checked >= 50, "至少要跑过 50 个 seed，实跑 {checked}");
}

/// 增量引擎必须与全量重算在**每个 refresh 点**都一致，而不只是最终状态。
/// TransientDriftEngine 的存在就是为了证明这两者不是一回事（spec §9.1）。
#[test]
fn incremental_engine_matches_naive_recompute_at_every_refresh_point() {
    let db = gen_database(2);
    let case = gen_case(11, &db, &Domain::default(), 25, 120, Batching::Chunks(5));

    let mut inc = IncrementalEngine::new();
    let mut naive = NaiveRecompute::new();
    let bases: BTreeMap<String, ZSet> = case
        .initial
        .iter()
        .map(|(t, rows)| (t.clone(), ZSet::from_rows(rows.iter().map(|r| (r.clone(), 1)))))
        .collect();
    inc.create_view(&case.database, &case.query, &bases).unwrap();
    naive.create_view(&case.database, &case.query, &bases).unwrap();
    assert_eq!(inc.materialize().unwrap(), naive.materialize().unwrap(), "bootstrap 即不一致");

    for batch in case.batches() {
        for (table, raw) in &batch {
            inc.apply(table, raw).unwrap();
            naive.apply(table, raw).unwrap();
        }
        inc.refresh().unwrap();
        naive.refresh().unwrap();
        assert_eq!(
            inc.materialize().unwrap(),
            naive.materialize().unwrap(),
            "增量与全量重算在某个 refresh 点分叉"
        );
    }
}

/// spec §5.2 的边界校验必须在 create_view 处生效，而不是等到 refresh 时 panic。
#[test]
fn create_view_rejects_a_global_aggregate() {
    let db = gen_database(1);
    let bad = ViewQuery {
        group_by: vec![],
        aggs: vec![Agg { func: AggFn::Count, column: None }],
        predicate: Predicate::None,
    };
    let mut engine = IncrementalEngine::new();
    let err = engine
        .create_view(&db, &bad, &BTreeMap::from([(db.tables()[0].table.clone(), ZSet::new())]))
        .expect_err("空 group_by 必须在 create_view 处被拒绝");
    assert!(
        err.0.contains("GROUP BY") || err.0.contains("group_by"),
        "错误应指名 group_by：{}",
        err.0
    );
}
```

> **`case.batches()` 本任务需要新增。** 目前 `batches(ops, batching)` 是
> `differential.rs` 里的私有自由函数，`run` 内部调用它。第二个测试要用两个引擎
> 并排走同一批序列，所以需要从外部拿到分批结果。加一个方法而非把自由函数改成
> `pub`：批次划分是 `TestCase` 的属性，方法形式让调用点读起来就是「这个用例的
> 批次」，也省得调用方自己把 `ops` 和 `batching` 配对（配错了不会编译失败，
> 只会静默地按错误的批次跑）。
>
> ```rust
> impl TestCase {
>     /// 本用例的分批结果，与 `run` 内部走的是同一条代码路径。
>     pub fn batches(&self) -> Vec<BTreeMap<String, Vec<(Row, i64)>>> {
>         batches(&self.ops, self.batching.clone())
>     }
> }
> ```
>
> `run` 内部改为调用 `case.batches()`，不要让两条路径各自分批——否则
> 「并排比对」比的可能是两种不同的批次划分，而那种失配不会有任何测试抓到。

> **`gen_case_with_query` 是本任务新增的生成器入口**（`crates/ivmlite-test/src/differential.rs`），签名 `pub fn gen_case_with_query(seed: u64, db: &Database, domain: &Domain, query: ViewQuery, rows_per_table: usize, op_count: usize, batching: Batching) -> TestCase`。现有的 `gen_case` 自己从 `enumerate` 里挑一个 query；要按枚举**逐个**覆盖查询空间就需要能指定 query。把 `gen_case` 实现成 `gen_case_with_query(seed, db, domain, enumerate(&db.tables()[0])[seed as usize % n].clone(), ...)`，两者共用同一条代码路径。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-test --locked incremental`
Expected: 编译失败，`cannot find type IncrementalEngine`

- [ ] **Step 3: 写实现**

`crates/ivmlite-core/src/engine.rs`：

```rust
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
```

`crates/ivmlite-test/src/incremental.rs`：

```rust
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
        Ok(IncrementalEngine::materialize(self))
    }
}
```

`crates/ivmlite-core/src/lib.rs` 与 `crates/ivmlite-test/src/lib.rs` 分别导出。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过。**若差分测试红了，那是引擎真有 bug——去查，不要调宽断言。** 失败信息会打印 `IVMLITE_SEED=N` 供精确重放。

- [ ] **Step 5: 变异验证**

1. `create_view` 里 bootstrap 的循环改成只处理 `db.tables()[0]` → 期望**仍绿**（Phase 1 已登记：查询只渲染 anchor 表）。这正是 join 落地清单上那三条的引擎侧对应物，**在门禁表里新增一行指回那份清单**，不要重复登记成一条独立缺口
2. `refresh` 里的 `std::mem::take` 改成 `clone`（pending 不清空）→ 期望 `incremental_engine_matches_naive_recompute_at_every_refresh_point` 红（delta 被重复应用）
3. `apply` 在 `tree.is_none()` 时返回 `Ok(())` 而非报错 → 期望**仍绿**（harness 从不在 create_view 之前调 apply）。记为「已知不被守护」，理由是它守的是误用而非语义
4. `materialize` 返回 `ZSet::new()` → 期望差分测试大面积红
5. `lower` 的错误不再向上传（`create_view` 里 `.unwrap()`）→ 期望 `create_view_rejects_a_global_aggregate` 红（变成 panic 而非 Err）

- [ ] **Step 6: 登记门禁并提交**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ docs/mutation-gates.md
git commit -m "feat: 增量引擎接入差分框架——M1a 检查点"
```

---

## Task 6：delta consolidation

**Files:**
- Modify: `crates/ivmlite-core/src/engine.rs`
- Modify: `docs/mutation-gates.md`

**Interfaces:**
- Produces: `IncrementalEngine::rows_processed_last_refresh(&self) -> usize`

> spec §8.2 把 consolidation 列为 **M1 的内容而非后续优化**，§8.5 称它是「本项目最可能成立的性能故事」。§8.5 同时指出：如果 harness 替引擎合并好了，consolidation 就从接缝上**结构性不可见**——引擎做没做，测试结果都一样。`apply` 收 raw Δ 正是为了让它可见，而本任务要让它**可测**。

### 为什么需要一个计数器

Consolidation 不改变结果，只改变工作量。于是它天然不可由「输出对不对」观察到——这正是 §8.5 警告的那种结构性不可见。

对策是让引擎公开一个统计量：上一次 `refresh` 实际推进算子树的行数。合并生效时，同一行在一批里出现 5 次只会被推进 1 次；`+1` 与 `-1` 相消的行会被推进 0 次。这两个数字是 consolidation 唯一的可观测足迹。

这不是测试专用的后门：写放大是 §11 的 M1 完成判定之一（「写放大有明确数字」），这个计数器正是那个数字的来源。

- [ ] **Step 1: 写失败的测试**

`crates/ivmlite-core/src/engine.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Agg, AggFn, Column, ColumnType, Predicate, Schema, Value};

    fn db() -> Database {
        Database::new(vec![Schema {
            table: "t".into(),
            columns: vec![
                Column { name: "k".into(), ty: ColumnType::Text, nullable: true },
                Column { name: "v".into(), ty: ColumnType::Integer, nullable: true },
            ],
        }])
    }

    fn query() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg { func: AggFn::Count, column: None }],
            predicate: Predicate::None,
        }
    }

    fn engine() -> IncrementalEngine {
        let mut e = IncrementalEngine::new();
        e.create_view(&db(), &query(), &BTreeMap::from([("t".to_string(), ZSet::new())]))
            .unwrap();
        e
    }

    fn row(k: &str, v: i64) -> Row {
        Row::new(vec![Value::Text(k.into()), Value::Int(v)])
    }

    #[test]
    fn duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators() {
        // spec §8.2/§8.5：同一行在一批里出现 5 次，合并后只推进 1 次。
        // 这是 consolidation 唯一的可观测足迹——它不改变结果，只改变工作量。
        let mut e = engine();
        let raw: Vec<(Row, i64)> = (0..5).map(|_| (row("a", 1), 1)).collect();
        e.apply("t", &raw).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            1,
            "5 条相同的 raw Δ 必须先合并成 1 条再进算子"
        );
        assert_eq!(
            e.materialize().weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(5)])),
            1,
            "合并不得改变结果：COUNT 仍是 5"
        );
    }

    #[test]
    fn rows_that_cancel_within_a_batch_never_reach_the_operators() {
        // 同一批里插入又删除同一行，合并后净权重为 0，根本不该进算子。
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 0, "相消的行不得进算子");
        assert!(e.materialize().is_empty());
    }

    #[test]
    fn distinct_rows_are_not_over_merged() {
        // 反向守护：合并不得把不同的行并成一条。
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1), (row("b", 1), 1), (row("a", 2), 1)])
            .unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 3, "三行互不相同，一条都不该被并掉");
    }

    #[test]
    fn deltas_for_different_tables_are_consolidated_separately() {
        // 同样的行值出现在两张表里时不得跨表合并——那会让一张表的变更
        // 消掉另一张表的变更。单表时这个形态不存在，join 落地后是常态。
        let two = Database::new(vec![
            Schema {
                table: "t".into(),
                columns: db().tables()[0].columns.clone(),
            },
            Schema {
                table: "u".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let mut e = IncrementalEngine::new();
        e.create_view(
            &two,
            &query(),
            &BTreeMap::from([("t".to_string(), ZSet::new()), ("u".to_string(), ZSet::new())]),
        )
        .unwrap();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.apply("u", &[(row("a", 1), -1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(
            e.rows_processed_last_refresh(),
            2,
            "两张表各自一条，不得跨表相消"
        );
    }

    #[test]
    fn the_counter_resets_between_refreshes() {
        let mut e = engine();
        e.apply("t", &[(row("a", 1), 1)]).unwrap();
        e.refresh().unwrap();
        e.apply("t", &[(row("b", 1), 1)]).unwrap();
        e.refresh().unwrap();
        assert_eq!(e.rows_processed_last_refresh(), 1, "计数是「上一次 refresh」而非累计");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p ivmlite-core --locked engine`
Expected: 编译失败，`no method named rows_processed_last_refresh`

- [ ] **Step 3: 写实现**

`IncrementalEngine` 加字段与方法，`refresh` 改为先合并：

```rust
    /// 上一次 `refresh` 实际推进算子树的行数。
    ///
    /// consolidation 不改变结果、只改变工作量，于是它无法由「输出对不对」
    /// 观察到——spec §8.5 把这种情形称作结构性不可见。这个计数器是它唯一的
    /// 可观测足迹，也是 §11 要求的「写放大有明确数字」的来源。
    rows_processed: usize,
```

```rust
    pub fn rows_processed_last_refresh(&self) -> usize {
        self.rows_processed
    }

    pub fn refresh(&mut self) -> Result<(), EngineError> {
        let tree = self
            .tree
            .as_mut()
            .ok_or_else(|| EngineError("refresh 在 create_view 之前被调用".into()))?;

        // spec §8.2：raw Δ 先按 Z-set 合并同一行的权重，再进算子。
        // 按表分别合并——同样的行值出现在两张表里时跨表相消是错的。
        // `BTreeMap` 而非 `HashMap`：推进顺序进入 delta 流（§9.4）。
        let mut by_table: BTreeMap<String, ZSet> = BTreeMap::new();
        for (table, row, w) in std::mem::take(&mut self.pending) {
            by_table.entry(table).or_default().update(row, w);
        }

        self.rows_processed = 0;
        for (table, delta) in &by_table {
            // 合并后净权重为 0 的行已被 `ZSet::update` 删掉（§5.1），
            // 于是它们根本不会出现在这里。
            self.rows_processed += delta.len();
            if delta.is_empty() {
                continue;
            }
            self.view.merge(&tree.delta(table, delta));
        }
        Ok(())
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test --workspace --locked --no-fail-fast`
Expected: 全部通过，**包括 Task 5 的全部差分测试**——consolidation 不得改变任何结果。这一点本身就是它正确的必要条件。

- [ ] **Step 5: 变异验证**

1. 去掉合并（退回 Task 5 的逐条推进）→ 期望 `duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators` 与 `rows_that_cancel_within_a_batch_never_reach_the_operators` 红
2. 合并不按表分组（全部并进一个 `ZSet`）→ 期望 `deltas_for_different_tables_are_consolidated_separately` 红
3. `self.rows_processed = 0` 那行删掉（改成累计）→ 期望 `the_counter_resets_between_refreshes` 红
4. `self.rows_processed += delta.len()` 改成 `+= 1`（每表记 1）→ 期望 `distinct_rows_are_not_over_merged` 红
5. `by_table` 的 `BTreeMap` 换 `HashMap` → 期望**仍绿**（单表时只有一个键；多表时算子对表的推进顺序目前不可观察）。**这一条必须记为「已知不被守护」并写进 join 落地清单**：join 落地后两侧 arrangement 的更新顺序会影响 `ΔR⋈ΔS` 项，届时必须重跑确认它转红

- [ ] **Step 6: 登记门禁并提交**

```bash
python3 scripts/count-mutation-gates.py --fix && python3 scripts/count-mutation-gates.py
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/ivmlite-core/src/engine.rs docs/mutation-gates.md
git commit -m "feat(core): delta consolidation——合并后再进算子"
```

---

## Self-Review

**1. Spec 覆盖**

| Spec 要求 | 落点 |
|---|---|
| §5.2 plan IR 五个算子 | Task 1（IR）、Task 3（Scan/Filter/Project）、Task 4（Aggregate）。**Join 不在本计划**——Phase 3，理由见开头范围说明 |
| §5.2 根算子必须是 Aggregate、GROUP BY 非空 | Task 1 的 `lower` 边界校验；同时把 m6 那条已登记缺口从「不适用」改成「已验证」并从 join 清单划掉 |
| §5.2 禁止全局聚合 | Task 1 的 `empty_group_by_is_rejected_at_the_boundary`；Task 5 的 `create_view_rejects_a_global_aggregate` 守边界的**入口** |
| §6.1 线性算子 Δ(f(R)) = f(ΔR) | Task 3 |
| §6.1 SUM 无非 NULL 输入返回 NULL | Task 4，`sum_over_only_null_inputs_is_null_not_zero` 与其反向 `sum_that_genuinely_totals_zero_is_int_zero_not_null` 成对 |
| §6.1 三值逻辑 | Task 3 的 `filter_treats_null_as_unknown_not_as_false_negation`；注释明写不得据此认为 `NOT p == !p` |
| §6.1 整数溢出未定义 | 不实现，由生成器的窄值域保证（`domain_cannot_overflow_integer_sum` 已守）。本计划不加代码，**这是有意的**：spec 明写 v0 的对策是夹住值域而非检测 |
| §6.2 retraction 对 | Task 4，五个测试分别守：改变发对、不变不发、清空只撤回、NULL 回退、输出权重恒为 1 |
| §6.3 Arrangement key → 多值、object-safe | Task 2 |
| §8.2/§8.5 consolidation | Task 6，用 `rows_processed_last_refresh` 让它可观测 |
| §8.5 apply/refresh 分离、apply 带表名、apply 收 raw Δ | Task 5 的适配器；三条签名一律不动 |
| §9.1 逐 refresh 点比对 oracle | Task 5 借 `run` 得到；`incremental_engine_matches_naive_recompute_at_every_refresh_point` 额外与参照实现逐点比 |
| §9.4 顺序确定 | Task 2（`scan` 有序）、Task 4（组按 key 排序发射）、Task 6（`by_table` 用 `BTreeMap`） |
| §11 M1a 检查点「单表引擎跑绿」 | Task 5 |
| §11 benchmark、写放大数字 | **不在本计划**——M0 的两个对照组都跑在 SQLite 里，与纯内存引擎不可比；Task 6 的计数器是写放大数字的来源，真正出数要等 M1b |

**已知未覆盖且有意为之**：`Arrangement` 在本计划里被 Task 2 建出来但**算子还没用到它**——v0 的 Aggregate 用 `BTreeMap` 持组状态就够了，`Arrangement` 的消费者是 join 的两侧（Phase 3）与 M1b 的 shadow table 实现。这是一个「实现了但没有消费者」的形态，正是本项目反复在压的那一类。**Task 2 的门禁行必须如实写明这一点**：`MemArrangement` 当前只被它自己的单元测试覆盖，没有任何算子消费它；Phase 3 的 join 是第一个消费者，届时必须重跑 Task 2 的全部变异确认它们仍然会红。这条也要进 join 落地清单。

> 为什么仍然现在就做：spec §6.3 明写「M0 不允许做出任何会导致加入 join 时返工的设计决定，`Arrangement` 的 key → 多值形状是这条约束的主要落点」。把 trait 的形状定下来并用内存实现验证它可用，成本是一个任务；等到 Phase 3 再定，join 的调试会和 trait 形状的调试纠缠。

**2. Placeholder 扫描**：无 TBD / TODO。每个 Step 3 都给出完整可编译代码。Task 3 的 `Node` 枚举在 Task 4 增加 `Aggregate` 变体——这是有意的增量，Task 3 里有一个显式的占位测试 `building_an_aggregate_is_an_error_until_task_4` 说明当前状态，Task 4 明确要求删掉它并给出替代测试。

**3. 类型一致性**：`lower(&ViewQuery, &str, usize) -> Result<Plan, PlanError>`（Task 1）→ `Node::build(&Plan) -> Result<Node, NodeError>`（Task 3）→ `AggState::new(Vec<usize>, Vec<Agg>)`（Task 4）→ `IncrementalEngine::create_view(&Database, &ViewQuery, &BTreeMap<String, ZSet>)`（Task 5）。`ivmlite-core::EngineError` 与 `ivmlite-test::EngineError` 同名不同类型，Task 5 的适配器显式搬运——这一点在 Task 5 的 Interfaces 里点明了。

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-21-m1a-phase2-engine.md`.
