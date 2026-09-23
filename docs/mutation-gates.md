# 变异门禁：spec 要求 → 变异 → 会红的测试

## 为什么有这份表

M0 结束时的最终全分支评审用**变异测试**——把实现改坏、看测试会不会红——在约一秒一次的运行里找到三处缺口：shrinker 的合法性门禁（spec §9.3 称之为不用 proptest 的全部理由）打桩成 `true` 后全套仍绿；bootstrap 之后的 oracle 比对整行删掉仍绿；`IsNotNull` 谓词的整个生成循环删掉仍绿。

在那之前，**十三轮基于阅读的逐任务评审一条都没抓到**。这不是评审者不认真——同一批评审在别处抓出了八条实质问题。差别在方法：读代码能判断"这段写得对不对"，判断不了"这段没了会不会有人发现"。

这份表把后一个问题变成机械检查。**每一条 spec 强制的不变量都要有一行**，写明：改坏什么、哪个测试会红。M1 的计划直接要求新增的不变量必须在此登记；评审时对表即可，不必考古。

## 怎么用

- **新增一条 spec 强制的行为时**，同时在此加一行，并真的跑一次变异确认测试会红。
- **"已验证"一栏只填真跑过的。** 推断出来的写"未验证"，不要把推断写成事实——那正是这份表要防的错误类型。
- **写进单元格的数字必须是从终端读回来的，不是算出来的。** M1a Phase 2 Task 2
  有一行写着某变异下「全绿（134/134）」，而实际是 128/128——实现者自己的报告里
  写的就是 128。追问来历后得到的机理很具体：当时正在同一份文档里新增 6 行门禁，
  于是把行数和测试数混在一起，算出 128 + 6 = 134，再把这个**算出来的**数字当成
  观测到的事实写了进去。不是手误（没有一个正确的 134 可供抄错），也不是标注过的
  估计。一个从未被观察到的数字，躺在一份以「让声称可被机械核对」为全部目的的
  文档里，正是它要防的东西。计数脚本抓不到这一类——它只数行的分类，不看格子里
  嵌的数字。
- 变异后**务必确认代码仍能编译**。我自己踩过一次：注入的变异语法错误，`grep` 把编译错误滤掉了，"没有输出"看起来和"测试通过"一模一样，差点把一个空的验证记成有效。
- 跑全套约 1.5 秒（首次构建后）。变异验证是廉价操作，不要因为"跑测试贵"而跳过——**那个假设我们已经错过一次**，它让十三轮评审白白放弃了最有效的工具。
- **必须用 `cargo test --workspace --locked --no-fail-fast`。** 默认的 `cargo test --workspace` 在第一个 crate 失败后就停，而 crate 的构建顺序与依赖顺序一致——于是下游 crate 里一个**顺带**被变异影响到的测试会先红并掩盖掉真正的目标。实测：把 `validate` 的 `>` 改成 `>=` 之后，默认命令只显示 `cells_reproduce_exactly_the_published_csv_matrix` 红，而该行具名的守护测试 `base_rows_equal_to_group_cardinality_is_accepted` 根本没跑到。
  这种掩盖**比"什么都没红"危险得多**：确实有东西红了，审计者会据此把该行标成已验证，依据却是一个表里没写的测试。它伪装成成功。

---

## ivmlite-core

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §5.1 权重归零的行必须删除，不留僵尸行 | 让 `ZSet::update` 在归零时保留条目 | `zero_weight_rows_are_removed_not_kept` | **已验证** |
| §5.1 负权重在中间 delta 中合法 | 让 `update` 钳制负权重 | `negative_weights_are_representable` | **已验证** |
| §9.4 迭代顺序确定（失败用例须可凭 seed 重放） | `BTreeMap` 换 `HashMap` | `iteration_order_is_deterministic` | **已验证** |
| §5.1 `Value` 无 Real/Blob（浮点结合律 / 整数溢出顺序依赖） | 加 `Real` 变体 | 编译失败（类型系统即门禁） | 不适用 |
| §5.2 根算子必须是聚合、`GROUP BY` 非空。**边界校验在 `lower`**（M1a Phase 2 Task 1）——此前只在生成器侧成立，由最终评审作为 m6 登记为「不适用」并排进 join 落地清单；`lower` 是引擎第一次真正消费 `ViewQuery`的入口，于是提前关掉了，清单里那一条也随之删除 | 删掉 `lower` 里的 `group_by.is_empty()` 校验 | `empty_group_by_is_rejected_at_the_boundary`（`crates/ivmlite-core/src/plan.rs`；此前这条约束只在生成器侧成立——唯一的生产者 `enumerate` 从不产出空 `group_by`，`enumerate_covers_the_v0_space_and_is_nonempty` 守的是这一点——但 `ViewQuery` 本身可以在 `enumerate` 之外自由构造。M1a Phase 2 Task 1 的 `lower` 是引擎第一次真正消费 `ViewQuery` 的入口，边界校验现在就在这里，不再只是生成器侧的偶然结果） | **已验证** |
| §5.2 根算子的 `Aggregate` 必须至少带一个 agg——没有 agg 的 "Aggregate" 实际是 `Scan`→`Project` 直接成为视图，Z-set 权重与 SQL 行数在该形状下语义不一致 | 删掉 `lower` 里的 `query.aggs.is_empty()` 校验 | `empty_aggs_is_rejected_at_the_boundary` | **已验证** |
| `lower` 必须在下标越界（`group_by` / agg 列 / 谓词列引用的下标 ≥ `arity`）时立即报错，而不是留到 `refresh` 时 panic——**这一行只证明"三处 `check(...)` 一起删掉会被抓到"**，不证明三处各自都被单独守着（最终评审 Finding C：`lower` 里实际有三处独立的 `check(...)` 调用——group_by / agg 列 / predicate 列——这条变异把三处一次性删光，而 `out_of_range_column_is_rejected` 用的用例是 `group_by=[7]`，agg 与 predicate 都在范围内；手术刀式地只删掉 agg 列或只删掉 predicate 列那一处（保留另外两处），这个测试依然全绿。真正分别钉住另外两处的是下面新增的两行） | 删掉 `lower` 里全部 `check(...)` 调用 | `out_of_range_column_is_rejected`（仅证明 group_by 那一支被守住；agg 列、predicate 列两支的守护见下面两行新增的门禁） | **已验证** |
| `lower` 里 agg 列的越界检查（`for agg in &query.aggs { if let Some(c) = agg.column { check(c, "agg")?; } }`）必须独立生效，不能靠 group_by 那处 `check` 顺带兜底（最终评审 Finding C：上一行的粗粒度变异一次删三处，掩盖了这一处单独没有测试守护这件事） | **只**删掉 agg 列那一处 `check(c, "agg")?`（保留 group_by 与 predicate 两处） | `out_of_range_agg_column_is_rejected`（`crates/ivmlite-core/src/plan.rs`；同一变异下 `out_of_range_column_is_rejected` 依然全绿，印证了它守不到这一处） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 168 passed / 1 failed（基线 169/0），红的正是新增的这一条 |
| `lower` 里 predicate 列的越界检查（`check(*column, "predicate")?`）必须独立生效，不能靠 group_by 那处 `check` 顺带兜底（最终评审 Finding C，同上） | **只**删掉 predicate 列那一处 `check(*column, "predicate")?`（保留 group_by 与 agg 两处） | `out_of_range_predicate_column_is_rejected`（`crates/ivmlite-core/src/plan.rs`；同一变异下 `out_of_range_column_is_rejected` 依然全绿） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 168 passed / 1 failed（基线 169/0），红的正是新增的这一条 |
| `Project` 收窄的投影列集合（group key ∪ 各 SUM 的列）不得重复——同一列既当 group key 又被 SUM 时只应投影一次，否则 `Project` 的输出行宽与 `Aggregate` 的下标重映射对不上 | `keep` 收集时去掉 `if !keep.contains(&c)` 去重判断，直接 `push` | `a_column_used_as_both_group_key_and_sum_target_is_projected_once` | **已验证** |
| `Scan` 必须取基表全部列——`lowers_to_scan_filter_project_aggregate` 对 `Scan.columns` 有一条直接的结构断言（`assert_eq!(columns, &vec![0, 1], …)`），这条断言真的守着 `Scan` 的列表本身。**最终评审 Finding G 补记：这条"要求"现在已经名不副实**——`node.rs` 的 `Node::build` 用 `Plan::Scan { table, .. }` 解构，`columns` 被 `..` 直接丢弃，`Node::Scan` 根本不持有它，`Node::delta` 更不可能按它来限定 `Scan` 该原样穿过哪些列。这一行的变异（把 `Scan.columns` 改成收窄后的 `keep.clone()`）依然会让 `lowers_to_scan_filter_project_aggregate` 变红，但那只是因为这条测试直接断言字段本身的值，不是因为字段被下游用来做了什么。这个字段目前是一份意图声明，不是一条生效的约束——它没有被删，是因为 M1b 的 delta-table reader 是它合理的第一个消费者（需要知道该 `SELECT` 哪些列），但在那之前不要把这一行的"已验证"读成"这个字段当前被谁依赖" | 把 `Scan` 的 `columns` 改成 `keep.clone()`（即在 `Scan` 处就收窄） | `lowers_to_scan_filter_project_aggregate`——它会变红，但原因**只是**上面那条对 `Scan.columns` 的直接结构断言；同一测试里更早的 `assert_eq!(predicate, &Predicate::IntGt { column: 1, value: 3 })` 在这个变异下**仍然通过**，因为 `Filter` 把 `query.predicate.clone()` 逐字存进节点，从不针对其 `input` 的列表重新索引或校验。**「`Filter` 必须按基表原始下标求值，而这只有在 `Scan` 吐出全部列时才成立」这条语义要求，今天没有任何测试覆盖，也覆盖不了**——`Plan` 目前没有任何消费者(求值器)，所以列下标语义是否用对根本不可观察。这条语义要求要到 Task 3 算子求值器落地后才第一次可证伪，届时必须在那次任务的门禁表里单独开一行、并真的跑一次变异验证；不能靠这一行顶替 | **已验证**（仅验证 `Scan.columns` 的结构断言本身；`Filter` 语义留给 Task 3）——**债已结清**：M1a Phase 2 Task 3 单独开了一行（见下方「Task 1 遗留债务，此处结清」），用 `Node`（`Plan` 的第一个消费者）真的把这条语义要求变成可证伪并跑了变异，见 `filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones`（`crates/ivmlite-core/src/node.rs`）。**但那笔债结清的是 `Filter` 的下标语义，不是 `Scan.columns` 这个字段本身**——Task 3 同时让这个字段变成了死数据（见本行前段），这一点当时没有记录，由最终评审 Finding G 补上 |
| `Predicate::None` 不应产生一个恒真的 `Filter` 节点——多一个节点就多一处每批都要走的无谓遍历，也会让「`Filter` 被正确跳过」这件事不可观察 | `Predicate::None` 时也插入 `Filter` 节点 | `no_filter_node_when_predicate_is_none` | **已验证** |
| `keep` 的去重必须以「这一列是否已经在 `keep` 里」为准，而不是「这一列是否等于某个 `group_by` 列」——两者只在被去重的列本身就是某个 group_by 列时才等价；`group_by=[0], aggs=[Sum(1), Sum(1)]` 时两个 agg 共用的列 1 根本不在 `group_by` 里，后一种判据永远为真，`Project.columns` 会变成 `[0, 1, 1]`——3 宽投影喂给一张 2 列的表（最终评审 Task 1 复审发现：`lower` 自己的文档注释拿"`ViewQuery` 可以自由构造"作为边界校验必须在此处的理由，而这正是那类构造） | agg 循环里的去重判据从 `!keep.contains(&c)` 改成 `!query.group_by.contains(&c)` | `two_aggs_sharing_a_non_group_by_column_are_projected_once` | **已验证** |
| §6.1 `Scan` 的 `delta` 必须按表名路由——只吸收自己那张表的 delta，单表时看似多余，但正是 join 两侧各自只吸收自己表 delta 的机制（Phase 3 不必改动 `Scan`，M1a Phase 2 Task 3） | `Node::Scan` 的 `delta` 去掉表名判断，恒返回 `input.clone()` | `scan_only_absorbs_its_own_table`（`crates/ivmlite-core/src/node.rs`） | **已验证** |
| §6.1 三值逻辑：谓词对 `NULL` 求值为 UNKNOWN，该行两边都不进结果——不是"NULL 等价于 false"，也不是"NULL 等价于 true"（M1a Phase 2 Task 3） | `passes` 对 `Value::Null`（`IntGt` 的非 `Int` 分支与 `IsNotNull`）都改成返回 `true` | `filter_treats_null_as_unknown_not_as_false_negation` 与 `is_not_null_predicate_filters_null_rows`（`crates/ivmlite-core/src/node.rs`） | **已验证** |
| §5.1 `Project` 收窄后与另一行重合时必须把权重相加，而不是后到的覆盖先到的——这是 Z-set 语义，也是 `Project` 唯一一处不平凡的地方（M1a Phase 2 Task 3） | `Project` 的 `out.update(narrowed, w)` 改成先写入一个临时 `BTreeMap` 再 `insert`（覆盖式写入，不经过 `ZSet::update` 的累加） | `project_merges_rows_that_become_identical_after_narrowing`（`crates/ivmlite-core/src/node.rs`；同一变异下 `project_drops_rows_whose_weights_cancel_after_narrowing` 也一并变红） | **已验证** |
| §6.1 线性算子 `Δ(f(R)) = f(ΔR)`：`Filter` 必须原样保留通过谓词的行的权重，含负权重（撤回一行满足谓词的行，撤回动作本身也要穿过去），不得把权重钳成常量（M1a Phase 2 Task 3） | `Filter` 的 `out.update(row.clone(), w)` 把 `w` 换成常量 `1` | `filter_passes_deltas_through_unchanged_for_matching_rows`（`crates/ivmlite-core/src/node.rs`） | **已验证** |
| `Project` 必须按 `columns` 给出的下标与顺序重排/收窄输出行，而不是原样传递整行（M1a Phase 2 Task 3） | `Project` 的 `columns.iter().map(...)` 改成直接克隆整行、忽略 `columns` | `project_narrows_columns_and_preserves_weights`（`crates/ivmlite-core/src/node.rs`；同一变异下 `project_drops_rows_whose_weights_cancel_after_narrowing`、`project_merges_rows_that_become_identical_after_narrowing`、以及下面「Task 1 遗留债务」一行的 `filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones` 也一并变红） | **已验证** |
| **Task 1 遗留债务，此处结清**（见上方 ivmlite-core 表「`Scan` 必须取基表全部列」一行）：`Filter` 的谓词必须按**基表**列下标求值，这只有在 `Filter` 位于 `Project` **之前**（因而还能看到收窄前的宽行）时才成立。`Plan` 在 Task 1 落地时还没有任何消费者，这条语义要求当时不可证伪（该行原文写明"要到 Task 3 算子求值器落地后才第一次可证伪"）；`Node`（本任务）是第一个消费者，这里第一次真正钉住它。**首版构造有缺口**（复审 Finding 1 发现）：`group_by=[0]`、谓词读列 1，Project 把行收窄到只剩 1 列，用错下标必然越界 panic——这钉住的是"下标别越界"，不是"谓词必须按基表下标求值"，一次把 `Row::get` 换成 `i.min(len-1)` 式钳制的未来重构会让它悄悄变绿而语义仍然是错的。**已改用**更强的构造：`arity=3`、`group_by=[2]`、`aggs=[Sum(1)]`、谓词读列 0，`keep=[2, 1]`——narrowed 行仍是 2 列宽，基表下标 0 与收窄后下标 0 指向两个都存在、但不同的列，用错下标不会 panic、只会算出错误答案。从 `lower()` 输出的 `Aggregate.input`（Task 3 尚未实现 `Aggregate` 节点，取其 input 即真实的 Filter/Project 子树）建 `Node`，喂两行 3 列宽的 delta（M1a Phase 2 Task 3；构造改于复审后） | 把 `lower()` 里包裹 `Filter` 与 `Project` 节点的先后顺序对调，使 `Project` 先包裹、`Filter` 后包裹（`Aggregate.input` 从正常的 `Project{ input: Filter{ input: Scan } }` 变成 `Filter{ input: Project{ input: Scan, columns: keep } }`） | `filter_evaluates_predicate_against_base_table_columns_not_narrowed_ones`（`crates/ivmlite-core/src/node.rs`；此变异下变红的是一个**断言失败**，不是 panic：`left: ZSet { inner: {Row([Int(200), Int(5)]): 1} }`、`right: ZSet { inner: {Row([Int(1), Int(5)]): 1} }`——两个都是合法形状的 `ZSet`，答案却不同，这正是本行要钉住的那类缺口。同一变异下 `plan::tests::lowers_to_scan_filter_project_aggregate` 的结构断言也一并变红） | **已验证** |
| `passes()` 里 `IntGt` 对非 `Int`（即 `Text`）值走的 `_ => false` 分支，其正确性论证与 NULL 分支不是同一类——NULL 分支的 `false` 是 spec §6.1 三值逻辑的正确答案；Text 分支的 `false` 只是一个**从未被任何路径触发**的死角，而且触发不到不是巧合：`crates/ivmlite-test/src/query.rs` 的 `enumerate`（第 41-47 行）只对 `int_cols` 里的列生成 `Predicate::IntGt`，Text 列永远不会走到这个分支（复审 Finding 2：此前源码注释误引 `enumerate_only_sums_integer_columns`，那条测的是 `SUM` 能作用于哪些列，与 `IntGt` 生成无关，已改正引用） | 只翻转 `IntGt` 匹配里 `Value::Text(_)` 这一支（不动 `Value::Null`，隔离出 Text 分支本身是否被守护），从 `false` 改成 `true` | 无——**已知不被任何测试守护，实测确认**：改后 `cargo build --workspace --locked` 编译通过，`cargo test --workspace --locked --no-fail-fast` 全绿（139/139）。差分层同样测不出来：`NaiveRecompute::passes`（`crates/ivmlite-test/src/naive.rs` 第 37-40 行）有一模一样的 `_ => false` 折叠，oracle 侧与引擎侧犯的是同一个错，`got == want` 会照样成立。**这个折叠不只是"没测到"，还与 SQLite 本身不一致**：SQLite 的类型排序是 `NULL < INTEGER/REAL < TEXT < BLOB`，实测 `SELECT 'abc' > 3` 在 SQLite 里返回 `1`（true），而这里返回 `false`——与 oracle 相反。今天无害纯粹是因为 `enumerate` 从不生成会触发这条分支的查询；如果这条生成规则将来放宽，v0 会在没有任何测试或差分比对能拆穿的情况下静默给出与 SQLite 相反的答案 | 不适用 — **superseded at the boundary (external review P2-1, 2026-09-22)**: `lower` now rejects `IntGt` over a non-INTEGER column, so this arm can no longer be reached through `create_view`. It stays reachable only by building a `Plan` by hand and calling `Node::build` directly, which bypasses `lower`; that is not an engine entry point. What was a reachability argument resting on `enumerate`'s convention is now an enforced boundary — see the three rows below. Supporting `IntGt` over TEXT would still require implementing SQLite's storage-class ordering (any TEXT > any INTEGER, regardless of content — measured `SELECT '0' > 3` = 1), not a string comparison |
| §6.1 `SUM` over a non-INTEGER column must be rejected at `create_view`, not answered differently from SQLite (external review P2-1). Measured on `STRICT` table `t(g INTEGER, v TEXT)`: SQLite's `SUM(v)` coerces numeric-looking text and returns `7.0` — a REAL — for rows `('7')`, `('abc')`; v0's `Value` has no `Real` variant and its accumulator only sees `Value::Int`, so it would report NULL | Remove the `require_integer(c, "SUM")?` call in `lower` | `sum_over_a_text_column_is_rejected_at_the_boundary` (observed 68 passed / 1 failed) | **已验证** |
| §6.1 `IntGt` over a non-INTEGER column must be rejected at `create_view` (external review P2-1): SQLite orders storage classes NULL < INTEGER/REAL < TEXT < BLOB, so `v > 3` is true for every TEXT value, and v0's `passes()` returns false | Remove the `require_integer(*column, "IntGt")?` call in `lower` | `int_gt_over_a_text_column_is_rejected_at_the_boundary` (observed 68 passed / 1 failed) | **已验证** |
| The type checks must not over-reject: `IS NOT NULL` is type-agnostic and must stay legal on TEXT columns | Make `lower` also require INTEGER for `IsNotNull` | `is_not_null_over_a_text_column_is_still_allowed` (68 passed / 1 failed), and also `incremental_engine_is_green_across_the_enumerated_space` plus `incremental_engine_satisfies_batch_invariance` (15 passed / 2 failed) — the enumerated v0 space uses `IS NOT NULL` on TEXT, so over-rejection is caught at both the unit and the differential level | **已验证** |
| §6.3 `Arrangement::get` 必须返回迭代器（key → 多值），不是 `Option`——v0 的 group-by 每个 key 只存一个值用不上，但 join 的每一侧都是 key → 多行，这个形状现在就必须成立 | `get` 里 `vals.iter()` 后加 `.take(1)`，只返回一个值 | `one_key_can_hold_multiple_values`（`crates/ivmlite-core/src/arrangement.rs`） | **已验证** |
| §5.1 权重归零的 (key,val) 必须删除，不留僵尸条目 | 删掉 `MemArrangement::update` 里 `if *w == 0 { vals.remove(val); if vals.is_empty() { self.inner.remove(key); } }` 整段 | `weights_accumulate_and_zero_removes_the_entry`（同一变异也会让 `a_key_with_no_values_left_disappears_from_scan` 一起红，因为两条不变量共用这段代码） | **已验证** |
| 值集合空掉后，key 本身也必须从 `inner` 里删除，否则 `inner` 的条目数会随历史（用过又清空的 key）而非当前状态增长 | 只删 `update` 里 `if vals.is_empty() { self.inner.remove(key); }` 这一行，保留 `vals.remove(val)` | `a_key_with_no_values_left_disappears_from_scan`——**注：brief 给的原始测试只断言 `scan()` 的输出，这个变异下该断言其实仍然是绿的**：`scan()` 用 `flat_map` 遍历 `inner`，一个空的内层 `BTreeMap` 天然贡献零条记录，不管外层 key 是否还留在 `inner` 里，所以这条不变量本来就不可能只靠 `Arrangement` 的三个公开方法观察到。已经在这个测试末尾加了一段白盒断言（直接查 `a.inner.contains_key(...)`），把它补成真正能红的守护 | **已验证**（加白盒断言后） |
| §9.4 `scan()` 的迭代顺序必须确定（失败用例要能凭 seed 精确重放） | 外层 `BTreeMap<Row, BTreeMap<Row, i64>>` 换成 `HashMap<Row, BTreeMap<Row, i64>>` | `scan_order_is_deterministic` | **已验证（统计性，非绝对）**——`HashMap` 的 `RandomState` 逐次构造重新播种，3 个 key 理论上约有 1/3! ≈ 16.7% 概率巧合排出正确顺序，不能保证每次都红。连续跑了 15 次独立进程（`cargo test -p ivmlite-core --locked arrangement::tests::scan_order_is_deterministic`），**15/15 全部变红**（含测试内 `assert_eq!(build(), build())` 这条同进程内两次调用互相比较的断言也失败，说明种子并非只按进程变化，逐次 `HashMap::new()` 都不同）。这是统计性守护，不是绝对保证——参照 `crates/ivmlite-core/src/database.rs` 的 `table_order_is_preserved` 一节的措辞 |
| §9.4 `scan()` 的输出不得依赖到达该状态所走的 update 历史——只检查"同一条代码路径重放两次自洽"和"key 有序"不够：两个用不同顺序（甚至含一段插入又撤回的弯路）到达完全相同最终状态的 `MemArrangement`，`scan()` 必须给出逐元素相同的输出，否则 delta 流不可能只凭最终状态和 seed 精确重放（复审 Finding 1 发现，`scan_order_is_deterministic` 测不出这一层） | 内层值容器从 `BTreeMap<Row, i64>` 换成插入序的 `Vec<(Row, i64)>`（线性查找/删除，外部行为不变） | `scan_order_is_independent_of_update_history`——`scan_order_is_deterministic` 对这个变异**仍然是绿的**：它只把同一条代码路径重放两次，两次的插入顺序完全相同，"自洽"这条断言天然测不出"输出依赖历史"这件事；升序插入 `[10,20,30]` 得到 `scan()` 顺序 `10,20,30`，降序插入（且中途插入又撤回一个无关值 5）得到 `30,20,10`，两者末状态完全相同但 `scan()` 输出不同 | **已验证** |
| `update` 里 `if weight_delta == 0 { return; }` 短路——纯属性能优化，没有可观察语义：`or_insert(0)` 之后再加 0、判零删除的逻辑与直接 `return` 在所有可观察行为上等价 | 删掉这一行 | 无——**已知不被守护**：删除后仍能编译，`cargo test --workspace --locked --no-fail-fast` 全绿（129/129） | 不适用 |
| `MemArrangement` 本任务落地后**暂无消费者**——v0 的 `Aggregate` 用普通 `BTreeMap` 存 group 状态，不经过 `Arrangement`；`Arrangement` 真正的消费者是 join 的两侧（Phase 3）与 M1b 的 SQLite shadow table 实现，本任务范围内没有任何算子依赖这个 trait | 不适用——没有集成层面的调用点可供变异，任何"删掉一处 `Arrangement` 用法"式的变异都无处下手 | 无——`MemArrangement` 目前只被它自己的单元测试覆盖（`crates/ivmlite-core/src/arrangement.rs` 的 `mod tests`），没有任何集成测试引用它，因此上面 5 行"已验证"的变异守护范围仅限于 `MemArrangement` 自身，任何真正跨算子的集成层面变异现在都测不到它。这 5 行本身也不是同一类：4 行（`get` 多值、归零删除、`scan` 顺序确定的两条）测的是 `Arrangement` 公开 trait 契约本身，只靠 `get`/`scan` 观察，原则上可以对任何实现重跑；1 行（空 key 是否留在 `inner` 里的白盒断言）直接查了 `MemArrangement` 的私有字段，绑死在这一个实现上，不可能通过 `dyn Arrangement` 或任何别的实现验证 | 不适用——见文末"Join 落地"清单第 4 条：Phase 3 join 落地、成为 `Arrangement` 第一个真实消费者时必须重新处理，但两类分开处理，不能一起打勾 |
| §6.2 聚合必须**撤回自己上一次发出的行**——SUM 从 100 变 150 时发的是 `(key,100) w=-1` 与 `(key,150) w=+1`，不是单独一行 `+1`。这是「聚合为什么需要状态」的真正原因：算子得记住自己发过什么才撤得回（M1a Phase 2 Task 4） | 删掉 `AggState::absorb` 发射块里的 `if let Some(old) = &g.emitted { out.update(old.clone(), -1); }` | `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert`（`crates/ivmlite-core/src/agg.rs`；同一变异下 `a_group_that_empties_is_retracted_and_not_replaced`、`a_group_whose_last_non_null_input_leaves_falls_back_to_null`、`groups_are_independent`、`sum_scales_each_input_by_its_weight` 也一并变红，共 5 条） | **已验证**——改后 `cargo build --workspace --all-targets --locked` 编译通过，`cargo test --workspace --locked --no-fail-fast` 得 144 passed / 5 failed（基线 149/0） |
| §5.2 边界校验：`AggFn::Sum` 必须带列——只有 `COUNT(*)` 允许不带列。此前 `agg.rs` 的 `expect("SUM 必须带列（lower 已校验）")` 声称 `lower` 已经查过，**那句是假的**（最终评审 F1 实测：`lower` 对 `Sum{column:None}` 返回 `Ok`，`Node::build` 也过，直到第一次 `delta` 才 panic）——`create_view` 成功之后的可达 panic，与 `plan.rs` 自己「引擎在 create_view 之后不应再有可预见的 panic 路径」的注释矛盾 | 删掉 `lower` 里 `agg.func == AggFn::Sum && agg.column.is_none()` 那个分支 | `sum_without_a_column_is_rejected_at_the_boundary`（实测 53 过 1 红） | **已验证** |
| §6.1 多个 agg 时，`accs[i]` 必须与 `aggs[i]` 一一对应，不能共用 `accs[0]` | 把发射侧的 `g.accs[i]` 全部改成 `g.accs[0]`（4 处） | `each_agg_reads_its_own_accumulator`（实测 53 过 1 红）。此前无任何测试覆盖：brief 里每个测试都只用一个 agg，而 `enumerate` 只产出 `[Sum(i)]` 或 `[Sum(i), Count]`（`Sum` 恒在 0 位），所以差分层同样抓不到；但 `lower` 接受 `[Count, Sum]`，`AggState::new` 也是公开的。该变异下 `[Count, Sum(1)]` 会静默发出 `SUM=NULL`，正是 §6.1 点名最危险的那一类 | **已验证** |
| §6.2 `AggState` 必须跨批存活——`emitted` 记的是「上一次发过什么」，只有跨批保留才撤得回上一批发出的行 | 在 `absorb` 开头插入 `self.groups.clear();` | `aggregate_retracts_across_two_batches_through_the_same_node` 等 6 个（实测 48 过 6 红） | **已验证**（但见下一行：这条变异证的是 `AggState` 层，不是节点层） |
| §6.2 同上的**节点层**形态：`Node::Aggregate` 必须持有状态而非每批重建 | 评审原本用的是 `let mut scratch = state.clone(); scratch.absorb(&upstream)`，但最终评审 F7 已把 `AggState` 上无人使用的 `Clone` 删掉，这个变异**现在写不出来**；节点层专属的变异需要从外部替换整棵树，而 `Node::Aggregate` 不持有重建所需的 `group_by`/`aggs`，为了造一个变异去加这些 API 是本末倒置 | 无——**已知不被节点层的变异守护**。上一行的 `groups.clear()` 会让那个两批测试变红，所以「状态丢失」这件事本身有守护，欠的是「丢失发生在节点层」这个特定形态。评审担心的真实重构（每次 refresh 重建算子树）发生在**引擎**里而非 `node.rs`：Task 5 的 `IncrementalEngine` 在 `create_view` 建树、`refresh` 复用，「`refresh` 改成重建树」是 Task 5 能表达且必须登记的变异。**这条债已在 Task 5 结清**，见下方 ivmlite-core 表末尾「§6.2 同上的**引擎层**形态」一行（`refresh` 从存下的 plan 重建树，而非复用 `self.tree`） | 不适用——债已转交并结清，见下方引擎层那行 |
| §5.2 `COUNT(*)` 必须等于组内**权重和**，不是组内 delta 行数——Z-set 的一行可以带任意权重，`rows += 1` 式实现在权重 ≠ ±1 或同组多行时立刻发散（M1a Phase 2 Task 4） | `AggState::absorb` 里 `g.rows += w` 改成 `g.rows += 1` | `an_unchanged_group_emits_nothing`（`crates/ivmlite-core/src/agg.rs`；同一变异下 `a_group_that_empties_is_retracted_and_not_replaced`、`emitted_output_weight_is_always_one` 也一并变红）。**这一行是为了让表与测试对上而补的**：brief 原本把 `an_unchanged_group_emits_nothing` 登记为「发射条件改成恒真」那条变异的守护测试，实测它守的不是那个（见下一行），而是这条 | **已验证**——编译通过，146 passed / 3 failed |
| §6.2 发射条件 `new_out != g.emitted`：输出没变的组不该发一对 `(-1, +1)` | `if new_out != g.emitted` 改成 `if true`（恒真） | 无——**已知不被守护，实测确认**：改后编译通过，`cargo test --workspace --locked --no-fail-fast` 全绿（149/149）。机理很干净：输出未变时撤回与重发的是**同一行**，`ZSet::update` 把 `-1` 与 `+1` 精确相消并删掉条目，于是多发的这一对在 `absorb` 的返回值里一点痕迹都不留；两个不同的 group 又必然产生不同的输出行（输出行以 group key 开头），不可能串台。brief 为这条变异指定的守护测试是 `an_unchanged_group_emits_nothing`，实测它在此变异下仍然绿——它真正守的是上一行的 `rows += w` | 不适用——**在当前形状下这是优化而非语义**：它省掉两次 `BTreeMap` 操作，可观察行为与恒真完全一致。brief 写的「这不是优化，是正确性」对**漏发**那一半成立（由上上行的撤回变异守着，删掉会让 5 条测试变红），对**多发**这一半不成立。一旦下游改成消费有序的 delta **序列**而不是 `ZSet`（相消不再发生），这一行就变得可证伪，届时必须补测试并把状态改成「已验证」 |
| §6.1（已实测）`SUM` 在「组非空但该列全为 NULL」时必须输出 `NULL`，而 SQLite 也输出 `NULL`；只维护累加值的实现会输出 `0`，且这个不一致是**静默**的——所以状态必须同时维护 `sum` 与 `non_null`（M1a Phase 2 Task 4） | 删掉 `g.accs[i].non_null += w` 这行（不再维护 `non_null`），输出判据从 `g.accs[i].non_null == 0` 改成 `g.rows == 0` | `sum_over_only_null_inputs_is_null_not_zero`（`crates/ivmlite-core/src/agg.rs`；同一变异下 `a_group_whose_last_non_null_input_leaves_falls_back_to_null` 也一并变红） | **已验证**——编译通过，147 passed / 2 failed |
| 与上一行相对：`SUM` 的判据必须是「有没有非 NULL 输入」，不是「和是不是 0」——有非 NULL 输入、其和恰为 0 时必须输出 `Int(0)` 而不是 `NULL`（M1a Phase 2 Task 4） | 输出判据 `if g.accs[i].non_null == 0` 改成 `if g.accs[i].sum == 0` | `sum_that_genuinely_totals_zero_is_int_zero_not_null`（`crates/ivmlite-core/src/agg.rs`） | **已验证**——编译通过，148 passed / 1 failed |
| `SUM` 必须按 `v * w` 累加输入权重，不是忽略权重直接 `+= v`（M1a Phase 2 Task 4） | `g.accs[i].sum += v * w` 改成 `g.accs[i].sum += v` | `sum_scales_each_input_by_its_weight`（`crates/ivmlite-core/src/agg.rs`）。**这条测试是本任务新写的**：brief 预测该变异会让 `emitted_output_weight_is_always_one` 或 `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert` 变红，实测两条都没红、全套 148/148 全绿——因为 brief 给的 SUM 测试输入权重全是 ±1（`v * w` 与 `v` 在 `w=1` 时相等），唯一带 `w=-1` 的那条又被「`non_null` 归零 → 输出 `NULL`」掩盖掉了。新测试用权重 3 的一行（必须贡献 15）再撤回 1 份（必须降回 10）把这个缺口补上 | **已验证**——编译通过，148 passed / 1 failed |
| §5.2 group key → 恰好一个输出行，对外 delta 的权重恒为 ±1；组内权重只活在算子状态里，不得泄漏到输出（M1a Phase 2 Task 4） | 发射新行的 `out.update(new.clone(), 1)` 权重改成 `g.rows` | `emitted_output_weight_is_always_one`（`crates/ivmlite-core/src/agg.rs`；同一变异下 `a_changed_sum_emits_a_retraction_pair_not_a_bare_insert`、`groups_are_independent`、`sum_over_only_null_inputs_is_null_not_zero`、`sum_scales_each_input_by_its_weight`、`sum_that_genuinely_totals_zero_is_int_zero_not_null` 也一并变红，共 6 条） | **已验证**——编译通过，143 passed / 6 failed |
| §9.4 Emit order must not depend on the arrival order of input rows. Since the external-review P2-2 fix (2026-09-22) the order comes from iterating `touched`, now a `BTreeSet`; the separate `keys.sort()` this row originally targeted no longer exists | Replace `touched`'s `BTreeSet` with a `HashSet` and `groups`' `BTreeMap` with a `HashMap` | None — **known to be unobservable, re-measured against the new code**: compiles, and the whole suite stays green (170/170) across 8 separate process runs. `absorb` returns a `ZSet`, whose `BTreeMap` makes the order of `update` calls on distinct rows irrelevant, and distinct groups always produce distinct output rows. Becomes falsifiable only if a downstream consumer takes an ordered delta sequence instead of a `ZSet` | 不适用 |
| Performance: `AggState::absorb` must not be quadratic in the number of distinct groups a batch touches (external review P2-2). `touched` was a `Vec` with a `contains` check per input row | Revert `touched` to `Vec<Row>` with `if !touched.contains(&key) { touched.push(key) }` | None — **known to be unguarded by any test, by design**: correctness tests cannot observe complexity, and a timing assertion would be flaky. Measured in release instead, 10k / 20k / 40k distinct groups: 67.3 / 255.9 / 1003.8 ms before (×3.8, ×3.9 per doubling — quadratic), 8.2 / 12.0 / 21.8 ms after. The differential suite's narrow value domain (a handful of groups per batch) could never have surfaced this; the M1b benchmark, which runs group cardinalities up to 100,000, is where a regression would show | 不适用 |
| 组彻底空掉（`rows == 0` 且 `emitted == None`）后从 `groups` 里删掉该组的状态 | 删掉 `if g.rows == 0 && g.emitted.is_none() { self.groups.remove(&key); }` 整段 | 无——**已知不被守护，实测确认**：改后编译通过，`cargo test --workspace --locked --no-fail-fast` 全绿（149/149） | 不适用——**这是内存回收，不是语义**：僵尸组的 `rows` 为 0、`emitted` 为 `None`，发射循环对它算出的 `new_out` 也是 `None`，于是 `new_out == g.emitted` 恒成立，它永远不可能再发出任何东西；唯一的可观察后果是 `groups` 的条目数随历史（用过又清空的 group）而非当前状态增长，而差分测试不测内存占用。与 `MemArrangement` 那条「值集合空掉后 key 也要删」的区别在于那里补了一条直接查私有字段的白盒断言；这里没补，因为 `AggState` 没有任何公开方法能观察 group 数，补一个只为测试存在的访问器不值当 |
| §6.2 `Node::Aggregate` 必须把上游 delta 真正喂给 `AggState`，而不是让它穿过去——聚合是本引擎唯一的有状态算子，`Filter`/`Project` 那条「delta 直接穿过」的规则对它**不**成立（M1a Phase 2 Task 4） | `Node::delta` 的 `Aggregate` 分支里 `state.absorb(&upstream)` 改成直接返回 `upstream`（加 `let _ = &state;` 避开未使用告警，确保变异能编译） | `aggregate_can_be_built_and_runs_through_the_tree`（`crates/ivmlite-core/src/node.rs`；这条测试替换了 Task 3 的占位测试 `building_an_aggregate_is_an_error_until_task_4`，走完整的 `Scan → Filter → Project → Aggregate` 一棵树） | **已验证**——编译通过，148 passed / 1 failed |
| §8.5 `IncrementalEngine::create_view` 声明了表却在 `initial` 里缺该表的状态时必须报错，不能悄悄用 `.take(1)` 式的短路把非 anchor 表当成"不用管"。**最终评审 Finding E：这一行此前登记的"实测"已经过期，是假的已验证**——`fe424e0`（本分支内、晚于这一行最初落笔）新增了 `create_view_errors_when_a_declared_table_has_no_initial_state`，它的 fixture 正是两张表、只给第一张的初始状态；这条测试恰好会被下面这条变异短路（`.take(1)` 之后循环压根不会走到第二张表，也就不会因为它缺初始状态而报错），但登记时没有人回来重新跑这条变异确认，白纸黑字写着"全绿（155/155）"。现在用当前代码重新测过：**这条性质确实被这个变异抓到了**——与下一行「非 anchor 表的 bootstrap 是否被正确吸收」是两条不同的性质，不能笼统合成一句"不适用" | bootstrap 循环 `for schema in db.tables()` 改成 `for schema in db.tables().iter().take(1)`，只吸收 anchor 表的初始状态 | `engine::tests::create_view_errors_when_a_declared_table_has_no_initial_state`——**且比预期红得更多，这是发现**：最终评审新增的 `engine::tests::a_failed_create_view_does_not_corrupt_existing_state`（Finding A 的守护测试，fixture 同样是两张表、只给第一张初始状态）也一并变红，原因相同（`.take(1)` 下 `create_view` 对这个 fixture 直接返回 `Ok`，两条测试的 `expect_err` 都落空）。实测：改后编译通过，`cargo test --workspace --locked --no-fail-fast` 得 167 passed / 2 failed（基线 169/0），红的正是这两条 | **已验证**——上一次登记时说的"全绿（155/155）"是过期的旧测量，当时 `create_view_errors_when_a_declared_table_has_no_initial_state` 尚未随 `fe424e0` 落地；现在这条测试的 fixture 天然会被 `.take(1)` 短路，重新测过后确认它会红 |
| 与上一行同一个变异（`.take(1)`）、但钉的是另一条不同的性质——文末「Join 落地」清单第 4 条（M1a Phase 2 Task 5 新增，第 1-3 条在**引擎侧**的同构对应物）：非 anchor 表的初始行必须真的被 bootstrap 吸收进 view，即使 `initial` 里每张表都给了状态。这条性质与上一行不同：上一行测的是"缺状态时报不报错"，这一行测的是"给了状态之后有没有真的被用上" | 同上——`for schema in db.tables()` 改成 `for schema in db.tables().iter().take(1)` | 无——**已知不被现有测试守护，实测确认**：改后编译通过，`cargo test --workspace --locked --no-fail-fast` 得 167 passed / 2 failed（基线 169/0）；这两条失败都来自上一行的性质（缺初始状态报错），没有任何测试单独因为"非 anchor 表的初始行没进 view"而红——根因与文末清单第 1-3 条相同：Phase 1/2 起，查询与 oracle 都只渲染 anchor 表的单表 SQL，非 anchor 表的状态在 `materialize()` 和 oracle 比对里天生不可观察 | 不适用——**并入文末「Join 落地」清单第 4 条，不单独登记为独立缺口**，join 落地、oracle 开始渲染多表查询时必须与前三条一起重新跑变异 |
| §8.5 `refresh` 必须清空 `pending`（`std::mem::take`），不能只是读它而不清（`clone`）——否则同一批 delta 会在下一次 `refresh` 时被重复推进算子树 | `refresh` 里 `std::mem::take(&mut self.pending)` 改成 `self.pending.clone()`（不清空） | `incremental_engine_matches_naive_recompute_at_every_refresh_point`（同一变异下 `incremental_engine_is_green_across_the_enumerated_space` 也一并变红） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 153 passed / 2 failed（基线 155/0） |
| `apply` 在 `create_view` 之前被调用时必须报错，不能悄悄放行（`self.tree.is_none()` 时返回 `Err`）——**M1a Phase 2 Task 5 复审 Finding 2 后补的单元测试**：原先登记为「不适用」（差分 harness 从不先 apply 后 create_view，判定为守的是误用顺序、不值得专门测），复审要求"要么测、要么点名"这条标准不能靠"harness 走不到"来豁免——`engine.rs` 自己加一条不依赖 harness 用例分布的直接单元测试成本很低，没有理由不做 | `apply` 里 `if self.tree.is_none() { return Err(...) }` 改成 `return Ok(())` | `engine::tests::apply_before_create_view_is_an_error`（`crates/ivmlite-core/src/engine.rs`） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 157 passed / 1 failed（基线 158/0） |
| `refresh` 在 `create_view` 之前被调用时必须报错，不能悄悄放行（`self.tree.as_mut()` 为 `None` 时返回 `Err`）——与上一行同一类、同一次复审新增（M1a Phase 2 Task 5 复审 Finding 2） | `refresh` 里 `self.tree.as_mut().ok_or_else(...)?` 改成 `let Some(tree) = self.tree.as_mut() else { return Ok(()); };` | `engine::tests::refresh_before_create_view_is_an_error`（`crates/ivmlite-core/src/engine.rs`） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 157 passed / 1 failed（基线 158/0） |
| **M1a Phase 2 Task 5 复审 Finding 2**：`create_view` 声明了一张表却在 `initial` 里找不到它的状态时必须报错，不能悄悄当空表处理——差分 harness 的 `gen_initial` 总是给 `db.tables()` 每一张表都填数据，这条路径在差分层结构性不可达；M1b 手工构造 `initial`（比如只为部分表重建视图）会真的踩上它。`engine.rs` 落地时该分支**零 `#[test]`**，唯一的说明性注释还声称与 oracle 侧 `missing_base_state_for_a_declared_table_is_an_error` 同一条约定——那条测试是真的，这条却没有，注释因此误导读者以为它也被守着 | `initial.get(&schema.table).ok_or_else(\|\| EngineError(...))?` 改成 `initial.get(&schema.table).unwrap_or(&empty)`（`empty` 是提前声明的 `ZSet::new()`，缺失的表悄悄当空表处理） | `engine::tests::create_view_errors_when_a_declared_table_has_no_initial_state`（`crates/ivmlite-core/src/engine.rs`；差分层的 15 个集成测试对同一变异**全部无感**——`harness_catches_bugs` 那个测试二进制仍是 15 passed / 0 failed，只有这条新单元测试红，印证了"差分 harness 走不到"这条诊断） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 157 passed / 1 failed（基线 158/0） |
| `IncrementalEngine::snapshot`（复审 Finding 1 前名为 `materialize`，见下方改名行）必须返回累积到目前为止的 `view`，不能是恒定的空 `ZSet`——这是 `IncrementalEngine` 对外暴露状态的唯一出口 | `snapshot` 的 `self.view.clone()` 改成 `ZSet::new()` | `incremental_engine_is_green_across_the_enumerated_space` 与 `incremental_engine_matches_naive_recompute_at_every_refresh_point`（差分测试大面积红） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 153 passed / 2 failed（基线 155/0） |
| **M1a Phase 2 Task 5 复审 Finding 1**：`IncrementalEngine` 的固有 `materialize(&self) -> ZSet` 与 `Engine::materialize(&mut self) -> Result<ZSet, EngineError>` 同名——固有方法在方法解析里总是优先于同名 trait 方法，任何持有具体 `IncrementalEngine` 类型（而非 `impl Engine` 泛型）的调用点会悄悄调错方法且没有编译期信号。首次落地时这个遮蔽已经真实发生过一次，逼着 `harness_catches_bugs.rs` 用 UFCS 绕开。修法：固有方法改名为 `snapshot`，把根因（重名）设计掉而不是在调用点绕 | 编译期设计约束，非运行时行为——没有"改坏它会红的测试"这个形状：`Engine::materialize` 与 `IncrementalEngine::snapshot` 现在是两个不同的名字，遮蔽在类型系统层面已经不可能发生（把 `snapshot` 改回 `materialize` 会让 `harness_catches_bugs.rs` 里 `inc.materialize().unwrap()` 编译失败——`ZSet` 没有 `unwrap`——这是编译器在拒绝重新引入这个缺陷，不是一条会变红的测试） | 无——不是变异可验证的性质 | 不适用——**设计约束，用编译失败而非测试红线守护**：这一行记录的是"为什么改名"，不是一条可以跑变异的不变量；改名前的遮蔽本身也从未被任何测试直接抓到过，是复审读代码 + 论证 M1b 调用形状发现的 |
| §5.2 的边界校验必须在 `create_view` 处以 `Err` 的形式传给调用者，不能在内部 `.unwrap()` panic 掉——`lower` 的错误必须能被 `?` 一路带出去，而不是等到 `refresh` 才炸 | `create_view` 里 `lower(...).map_err(|e| EngineError(e.0))?` 改成 `lower(...).unwrap()` | `create_view_rejects_a_global_aggregate` | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 154 passed / 1 failed（基线 155/0） |
| §6.2 同上的**引擎层**形态（上方「§6.2 同上的节点层形态」一行把这条债交棒到此处）：`refresh` 必须复用 `create_view` 建好的 `self.tree`，不能每次都从 plan 重新 `Node::build`——重建会让 `AggState` 在两次 refresh 之间丢光状态，于是每次都从空 group 起算，永不撤回上一次发出的行，只发裸 `+1`（M1a Phase 2 Task 5） | 给 `IncrementalEngine` **临时**加一个 `plan_for_mutation_test: Option<Plan>` 字段（`create_view` 里连带存一份 `plan.clone()`），把 `refresh` 改成从这个字段 `Node::build` 出一棵全新的树、完全不碰 `self.tree`；验证完立刻把字段和改动一起还原，不进入生产代码——做法与上一行「评审原本用的是 `let mut scratch = state.clone()`」一致：只为跑通这条变异临时加，不是为了给变异专门扩大公开 API | `incremental_engine_matches_naive_recompute_at_every_refresh_point`（同一变异下 `incremental_engine_is_green_across_the_enumerated_space` 也一并变红） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 153 passed / 2 failed（基线 155/0）；变异还原、字段移除后重新跑同一条命令，155/155 全绿 |
| §8.2/§8.5 `refresh` 必须先按 `ZSet` 合并本批 raw Δ 再推进算子树，不能逐条推进（consolidation，M1a Phase 2 Task 6）——这是本条计划唯一在意的性能故事：合并不改变结果，只改变工作量，于是必须靠 `rows_processed_last_refresh` 这个计数器才可观测。**最终评审 Finding B：这一行只证明"整个 consolidation 机制被拆掉"会被抓到，不证明"计数器真的在观察算子树收到了什么"**——`rows_processed` 曾经由 `refresh` 在调用 `tree.delta` **之前**、从合并后的 `ZSet` 单独算出来（`self.rows_processed += delta.len();`），与紧接着的 `self.view.merge(&tree.delta(table, delta));` 之间只是相邻两行代码、没有任何数据依赖。这一行的变异（整体退回逐条推进）连带把计数逻辑也改掉了，所以能让这个计数器测试变红；但一个只改"喂给算子树的方式"、不动计数逻辑的手术刀式变异——见下面新增的一行——会证明这个计数器当时其实测不到"算子树被推进了几次"这件事，只测得到"合并前后的行数对不对"。下面那行是真正堵住这个缺口之后的变异证据 | `refresh` 退回 Task 5 的逐条推进：对 `pending` 里每一条 raw `(table, row, w)` 各建一个单行 `ZSet::from_rows` 并各自 `tree.delta`，`rows_processed` 每条 raw Δ 记 1（不做任何合并） | `engine::tests::duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators`、`engine::tests::rows_that_cancel_within_a_batch_never_reach_the_operators` | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 161 passed / 2 failed（基线 163/0，本轮复审前的旧基线），红的正是这两条、不多不少 |
| （最终评审 Finding B，堵住上一行的缺口）`refresh` 必须把本批合并后的**整个** `ZSet` 一次性交给算子树，不能拆成逐行调用——即使拆开之后总行数、最终结果都不变，调用次数本身也是 consolidation 承诺的一部分：`IncrementalEngine` 新增了 `CountingTree`，把 `pushes`（`Node::delta` 顶层入口被调用的次数）与 `rows_fed`（累计喂给它的行数）都改成从调用本身观察——`CountingTree` 的 `node` 字段私有，`refresh` 里除了 `CountingTree::delta` 没有第二条路径能摸到底下的 `Node`，`rows_processed`/`tree_pushes` 现在直接读 `tree.rows_fed`/`tree.pushes`，不再是 `refresh` 自己另算的数字 | 保留合并逻辑与最终结果不变，只把 `self.view.merge(&tree.delta(table, delta));` 改写成对 `delta.iter()` 逐行建单行 `ZSet` 各调一次 `tree.delta`（`for (row, w) in delta.iter() { let one = ZSet::from_rows([(row.clone(), *w)]); self.view.merge(&tree.delta(table, &one)); }`）——这正是复审给出的那个手术刀式变异，`rows_processed` 的值不受影响（求和不变），只有调用次数变了 | `engine::tests::distinct_rows_are_not_over_merged`（新增的 `tree_pushes_last_refresh() == 1` 断言；同一变异下 `engine::tests::deltas_for_different_tables_are_consolidated_separately`、`engine::tests::duplicate_rows_in_one_batch_are_merged_before_reaching_the_operators` 里同样新增的 `tree_pushes` 断言不会红，因为这两条测试合并后每张表都恰好只剩 1 行，`delta.iter()` 本来就只迭代 1 次——`distinct_rows_are_not_over_merged` 合并后仍有 3 个互不相同的行，是唯一能把"逐行 push"和"整批 push"从调用次数上区分开的用例） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 168 passed / 1 failed（基线 169/0），红的只有 `distinct_rows_are_not_over_merged`，失败信息为 `assertion `left == right` failed: 三行分属同一张表、同一次 refresh，只应向算子树推进一次，不是逐行 push\n  left: 3\n right: 1` |
| §8.2 同一行值出现在两张表里时不得跨表合并——按表分组合并是正确性要求，不只是性能优化：跨表相消会让一张表的变更悄悄抵消另一张表的变更 | `refresh` 不按表分组，取 `pending` 里第一条的表名当唯一 key，把全部原始 Δ（不分表）合并进同一个 `ZSet`，再用这一个 key 调一次 `tree.delta` | `engine::tests::deltas_for_different_tables_are_consolidated_separately` | **已验证——比预期红得更多，这是发现**：编译通过，`cargo test --workspace --locked --no-fail-fast` 得 158 passed / 3 failed（基线 163/0）。除了 brief 预测的那一条单测，`ivmlite-test` 差分套件里 `incremental_engine_is_green_across_the_enumerated_space` 与 `incremental_engine_matches_naive_recompute_at_every_refresh_point` 也一并变红——这两条内部都用 `gen_database(2)` 建真正的双表用例（`crates/ivmlite-test/tests/harness_catches_bugs.rs:341,362`），不像 m6 登记的 anchor-only 缺口那样被 oracle 单表渲染挡住；跨表合并把非 anchor 表的行错误地打上 anchor 表名喂进 `tree.delta`，被 `Scan` 当成合法输入吃进去，直接产出错误结果，而不是「结构性不可见」 |
| §11「写放大有明确数字」：`rows_processed_last_refresh` 必须反映「上一次」`refresh` 推进算子树的行数，不能累计多次 `refresh` | 删掉 `self.rows_processed = 0` 这一行归零，改成跨 `refresh` 累计 | `engine::tests::the_counter_resets_between_refreshes` | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 162 passed / 1 failed（基线 163/0） |
| §11 同上：计数必须是合并后实际推进的**行数**，不能是「有变更的表数」——否则一张表里 3 行不同的行和 1 行会显示成同一个数字，写放大就没法从这个数字读出来 | `self.rows_processed += delta.len()` 改成 `+= 1`（每张有变更的表只记 1，不管合并后剩几行） | `engine::tests::distinct_rows_are_not_over_merged` | **已验证——比 brief 预测的多红一条，这是发现**：编译通过，`cargo test --workspace --locked --no-fail-fast` 得 161 passed / 2 failed（基线 163/0）。除了 brief 点名的 `distinct_rows_are_not_over_merged`，`engine::tests::rows_that_cancel_within_a_batch_never_reach_the_operators` 也一并变红——该测试里合并后净权重为 0，`delta.len()` 是 0，但 `by_table.entry("t").or_default()` 已经在 `BTreeMap` 里建了一个空 `ZSet` 的 key，`+= 1` 对着这个空条目也记了 1，而正确实现的 `+= delta.len()` 在这里应得 0。两条测试断言的其实是同一处代码，不是巧合命中 |
| §9.4 合并按表分组用 `BTreeMap` 而非 `HashMap`——迭代顺序确定 | `by_table` 的 `BTreeMap` 换成 `HashMap` | 无——**已知不被现有测试守护，实测确认**：编译通过，`cargo test --workspace --locked --no-fail-fast` 全绿（163/163）。单表时 `by_table` 只有一个 key，顺序无意义；多表时 `for (table, delta) in &by_table` 的推进顺序目前只影响 `self.view.merge(...)` 调用的先后，而 `ZSet::merge` 是逐点加法、与调用顺序无关，所以当前没有任何观察点能看出这个顺序 | 不适用——**并入文末「Join 落地」清单第 5 条**：join 落地后 `ΔR⋈ΔS` 项会同时读两侧 arrangement 的当前状态，那时两侧更新顺序才第一次影响输出，必须重新跑这条变异确认它转红 |
| **最终评审 Finding A**：`create_view` 必须只在 bootstrap 循环**整体成功**之后才提交新状态——循环内对某张声明了却缺初始状态的表会 `?` 提前返回，提交提前发生会让 `self.view` 变成一个只吸收了部分表的半成品，而 `self.tree`（以及现在的 `self.tables`）还停在上一次成功的 `create_view` 建的那些值上，两者从此永久不一致且后续 `apply`/`refresh` 都不再报错，只会安静地算出错误答案 | 把提交顺序改回"先提交、后遍历"：`self.view = ZSet::new();` 挪到 bootstrap 循环**之前**直接写 `self`（而不是先建一个局部 `view` 变量，循环成功后再整体赋给 `self.view`） | `engine::tests::a_failed_create_view_does_not_corrupt_existing_state`（`crates/ivmlite-core/src/engine.rs`；同一变异下 `engine::tests::create_view_errors_when_a_declared_table_has_no_initial_state` 仍然全绿——它只断言"第二次 create_view 报错"这件事本身，不断言报错之后引擎状态有没有被污染，是两条独立的性质） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 168 passed / 1 failed（基线 169/0），红的只有新增的这一条。第二次 `create_view` 本身仍然按预期报错（`expect_err` 能过），真正变红的是随后的状态比较，实测 panic 信息：`assertion `left == right` failed: 失败的第二次 create_view 不得污染既有视图状态\n  left: ZSet { inner: {} }\n right: ZSet { inner: {Row([Text("a"), Int(1)]): 1} }`——`left` 是变异后被污染成空的视图，`right` 是失败前的正确基线 |
| **最终评审 Finding L**：`apply` 必须拒绝 `create_view` 未声明过的表名，不能来者不拒——引擎不持有 `Database`，此前对未知表名直接堆进 `pending`，`refresh` 时喂给 `Node::Scan`，`Scan` 只按表名路由、不认识的表名被它自己悄悄吃成一个空 delta，`apply` 因此"成功"了，视图却完全没被这次调用影响到 | 删掉 `apply` 里 `if !self.tables.contains(table) { return Err(...) }` 这一段校验 | `engine::tests::apply_rejects_an_unknown_table`（`crates/ivmlite-core/src/engine.rs`） | **已验证**——编译通过，`cargo test --workspace --locked --no-fail-fast` 得 168 passed / 1 failed（基线 169/0），红的正是这一条 |

## ivmlite-test：生成器

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §9.2 值域必须窄（否则测不到同组反复增删） | `Domain::default` 的 `distinct` 改大 | `domain_is_narrow_by_default` | **已验证** |
| §9.2 NULL 必须高频 | `null_rate` 默认值改动 | `domain_is_narrow_by_default`（含精确断言） | **已验证** |
| §6.1 整数值域须远离溢出 | 放宽 `Domain` 上界 | `domain_cannot_overflow_integer_sum` | **已验证** |
| §9.2 `distinct == 0` 不得 panic | 去掉 `.max(1)` | `zero_distinct_domain_does_not_panic` | **已验证** |
| §9.2 有偏采样：DELETE 必须命中存在的行 | `gen_ops` 的删改目标改为随机生成 | `deletes_target_rows_that_actually_exist` | **已验证** |
| §9.2 live 集合为空时只能插入 | 去掉 `live.is_empty()` 守卫 | `empty_live_set_only_ever_produces_an_insert_first` | **已验证** |
| §9.2 第 4 条：差分 schema 固定每表 2 列 | `gen_database` 的列数改成 3 | `generated_database_tables_have_exactly_two_columns` | **已验证** |
| §9.2 多表生成器必须给每张表都分配操作（否则 join 的 ΔR⋈S / R⋈ΔS 只有一条路径被测到） | `gen_ops` 只往 `db.tables()[0]` 写 | `every_table_receives_some_ops` | **已验证** |
| §9.2 选表必须均匀，不止"没完全排除某表"——偏斜到 90/10 也要被抓到 | 选表改成 90/10 偏向 `db.tables()[0]`（`tables.len() <= 1` 时退化为 0，兼容单表包装用例） | `every_table_receives_some_ops`（阈值须是 `count / db.len() / 2` 这类随均匀期望缩放的下界；原 `n > 20` 对 90/10 偏斜下少数表拿到的 41/300 仍判定通过，是评审发现的缺口） | **已验证** |
| §9.2 有偏采样必须按表各自维护 live 集合，不得跨表删改 | `gen_ops` 的删改目标改为跨表采样（从任意表的 live 集合里取） | `deletes_target_rows_that_exist_in_their_own_table` | **已验证** |
| §5.2 根算子必须是聚合、group_by 非空 | `enumerate` 产出无聚合或空 group_by 的 query | `enumerate_covers_the_v0_space_and_is_nonempty` | **已验证** |
| §6.1 `SUM` 只作用于整数列 | 让 `enumerate` 对 Text 列产出 `Sum` | `enumerate_only_sums_integer_columns` | **已验证** |
| §6.1 `COUNT(*)` 不带列 | 让 `enumerate` 产出 `Count` 带 `Some(_)` | `enumerate_always_count_has_none` | **已验证** |
| §6.1 谓词白名单三种形式均须被枚举 | 删掉 `IsNotNull` 的生成循环 | `enumerate_covers_the_v0_space_and_is_nonempty` | **已验证**（最终评审） |

## ivmlite-test：语义契约

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §7.1 建表必须 `STRICT` | 去掉 `STRICT` 后缀 | `create_table_sql_is_strict` | **已验证** |
| §6.1 `SUM` 无非 NULL 输入时返回 NULL 而非 0 | 发射分支改看 `total == 0` | `sum_that_totals_zero_is_int_zero_not_null` | **已验证** |
| §6.1 三值逻辑：NULL 谓词不入结果 | `passes()` 对 NULL 返回 true | `is_not_null_predicate_filters_out_null_rows` | **已验证** |
| §5.1 权重 ≤ 0 的行不参与重算 | 删掉 `weight <= 0` 守卫 | `retracting_a_row_that_was_never_inserted_is_a_noop` | **已验证** |
| §8.5 A reference engine reused across `create_view` calls must not carry unrefreshed deltas into the new view (external review P2-4, 2026-09-22) | Delete `self.pending.clear()` from `NaiveRecompute::create_view` | `recreating_a_view_discards_deltas_applied_but_not_refreshed` (observed 63 passed / 1 failed) | **已验证** |
| §9.1 oracle 须按权重展开行 | 每行只插一次 | `expands_rows_by_weight` | **已验证** |
| §9.1 基表状态出现负权重须报错而非静默 | 改为跳过负权重行 | `rejects_negative_weights_in_base_state` | **已验证** |
| §9.1 输出行宽须等于 `output_arity()` | 删掉宽度检查分支 | `rejects_wrong_row_width` | **已验证** |
| §9.1 group key 不得重复 | 删掉重复检查 | `rejects_duplicate_group_keys` | **已验证** |
| §9.4 oracle 须建出 `Database` 声明的每一张表，而非只建查询用到的那张 | 把建表循环限制成只处理 `db.tables()[0]` | `builds_every_table_in_the_database` | **已验证** |
| §9.4 同上：建表**本身**必须对每张表执行，而非只走到校验 | 只跳过非 anchor 表的 `CREATE TABLE`，保留基表查找与权重校验 | `builds_every_table_in_the_database`（断言须同时含表名与 `含负权重`；只查表名子串时此变异会因 `no such table: customers` 误判通过） | **已验证** |
| §9.4 声明了表却没给基表状态必须报错，而非静默当空表 | 缺表时退回 `unwrap_or(&ZSet::new())` 之类的静默兜底 | `missing_base_state_for_a_declared_table_is_an_error` | **已验证** |

## ivmlite-test：驱动与接缝

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §8.5 `apply` 收未合并的原始 Δ | 在 `batches()` 里重新折叠进 `ZSet` | `apply_receives_unconsolidated_raw_deltas` | **已验证** |
| §8.5 `apply` 带表名 | 传死值而非 `schema.table` | `apply_receives_the_schema_table_name` | **已验证** |
| §9.1 bootstrap 之后立即比对 oracle | 删掉 `compare(engine, &base, "bootstrap")` | `bootstrap_drift_is_caught_at_the_bootstrap_stage` | **已验证**（最终评审） |
| §9.1 每个 refresh 点都比对 oracle（非仅末尾） | 只在循环结束后比对一次 | `oracle_comparison_runs_after_every_batch_not_only_at_the_end`（缺口修复新增；原 `transient_drift_has_a_correct_final_state`——m5 更名前叫 `per_batch_oracle_comparison_catches_transient_drift`——的断言过松，实测这个变异不会让它变红，细节见 task-1-report.md） | **已验证** |
| §9.3 shrinker 的合法性门禁 | `is_legal` 函数体替换为 `true` | `dangling_delete_is_illegal` / `dangling_update_is_illegal` / `delete_after_insert_of_a_different_row_is_illegal` | **已验证** |
| §9.4 `IVMLITE_SEED` 非数字须 panic | 改为静默忽略 | `parse_seed_arg_panics_on_non_numeric_value` | **已验证** |
| §9.4 `batches()` 用 `BTreeMap` 分组必须真的产生确定的、与 `db.tables()` 一致的批内表顺序（I3，最终评审：原注释只声称"用 BTreeMap 保证确定"，此前没有测试钉死这句话本身；§7.2/§7.3 落地后这个顺序要喂 GC 与 bootstrap 水位，届时"顺序无关紧要"会变成"顺序决定 seed 能否重放") | 把 `batches()` 的返回类型与内部分组容器都换成 `HashMap` | `per_batch_apply_order_follows_db_tables_order`（对"继续用 `BTreeMap`"这一侧是绝对保证——`BTreeMap` 按 key 排序是标准库文档承诺的行为；对"换成 `HashMap` 后必然变红"这一侧是统计保证，因为 `HashMap` 的迭代顺序由每次构造时随机生成的 `RandomState` 决定，键数越少巧合排对的概率越高——连续重跑 5 次全部变红，但理论上不能排除某次运行偶然拿到正确顺序） | **已验证** |
| §9.1 植入 bug 的引擎必须保持有 bug 且能被抓到 | 修好 `NoRetractionEngine` | `harness_catches_the_missing_retraction_bug` | **已验证** |
| §9.1 漂移污染必须绕过不变量层 | 污染改为只在末列是 `Int` 时生效 | `drift_still_happens_when_the_aggregate_column_is_null` | **已验证** |
| §8.5 `apply` 必须把每批的 delta 按表路由到各自的基表，而非全部塞给同一张表（M1a Phase 1 Task 5：M0 只有一张表时这个参数形同虚设，多表化后才第一次真的需要路由） | 在 `run` 里把递给 `apply` 的表名改写死成 `db.tables()[0].table`（只改这一处引擎接缝，`bases` 的参照 bookkeeping 仍按原表名推进——两边都改会让 oracle 与引擎一起偏航、测不出任何东西） | `a_two_table_case_runs_green_against_the_reference_engine` | **已验证** |
| §8.2「N 次 apply、一次 refresh」：一批之内对多张表的 delta 只应触发一次 refresh（M1a Phase 1 Task 5） | 把 `run` 改成对批内每张表各调一次 `refresh`（而非批末统一调一次） | 无——**已知不被现有测试守护**：`NaiveRecompute::refresh` 只是把 `pending` drain 进 `base`，不在 `refresh` 内部做 consolidation，所以“更细粒度地调用 refresh”和“批末调一次”对它是同一件事，两种调用节奏产出完全相同的最终状态。真正能区分这条要求的是一个把 consolidation 逻辑放在 `refresh` 内部的引擎，属引擎计划（M1）范围，本计划不为此新增测试 | 不适用 |
| §9.3 shrinker 的合法性门禁必须**按表**校验：用 A 表当时存在的行去合法化对 B 表的 DELETE/UPDATE 是错误的（M1a Phase 1 Task 5：单表时这个形态根本不存在） | `is_legal` 的存在性检查改成跨全部表的行联合查找，而不是只看 `table` 自己的 live 集合 | `deleting_a_row_that_exists_in_another_table_is_illegal` | **已验证** |
| §8.5 `apply` 必须真的保留非 anchor 表的 delta（M1a Phase 1 Task 5 评审发现）：`NaiveRecompute` 按表持有 `base`/`pending` | 让 `NaiveRecompute::apply` 对 `table != self.anchor` 直接 `return Ok(())`，静默丢弃非 anchor 表的全部 delta | 无——**已知不被现有测试守护**：Phase 1 的查询与 oracle 都只渲染 anchor 表（`db.tables()[0]`）的单表 SQL，非 anchor 表存进 `base` 的状态在 `materialize()` 和 oracle 比对里都不可观察；唯一能抓到这条的测试得直接断言 `NaiveRecompute` 的内部字段，测的是实现而非行为，而且 join 落地后这个断言还得重写。跑变异实测：改后仍能编译，`cargo test --workspace --locked --no-fail-fast` 全绿（110/110），与预期一致 | 不适用 |
| §8.2 `run` 自己的参照 bookkeeping（`bases`，喂给 oracle 的那份状态）必须真的推进每一张非 anchor 表（最终评审 I2 发现）：上一行登记的是引擎侧 `NaiveRecompute` 会静默丢非 anchor delta；这一行是它的镜像——harness 侧自己维护的 `bases` 也可能犯同样的错，而且更要命，因为 `bases` 直接就是 oracle 的输入 | 在 `run` 的批循环末尾，把递给 `bases.entry(...)` 的按表更新改成只在 `table == anchor` 时才执行，非 anchor 表的 `bases` 记账被静默跳过（引擎侧 `apply` 收到的表名与 raw delta 不变，只改 harness 自己的参照 bookkeeping 这一处） | 无——**已知不被现有测试守护**，原因与上一行相同：Phase 1 的查询与 oracle 都只渲染 anchor 表的单表 SQL，`bases` 里非 anchor 表的值在 `recompute_via_sqlite` 的输出里不可观察，静默冻结它也不会让任何比对变红。跑变异实测：改后仍能编译，`cargo test --workspace --locked --no-fail-fast` 全绿（110/110） | 不适用 |
| §9.3 `shrink` 的 phase 3（逐表、逐行删初始数据）必须真的遍历 `best.initial` 的每一张表，而不只是循环第一次碰到的那张（I4，最终评审发现：评审用探针 `assert!(case.database.len() <= 1)` 证实此前没有任何调用点喂给 `shrink` 一个真正的多表用例，反转该循环的表迭代顺序也不会让任何测试变红） | 把 phase 3 的 `let tables: Vec<String> = best.initial.keys().cloned().collect();` 改成 `.take(1)`，只处理第一张表 | `shrink_reduces_initial_rows_in_every_table_of_a_multi_table_case` | **已验证** |
| §9.3 同上，补充说明：**反转**该循环的表迭代顺序（而非只处理第一张表）不属于这一行的守护范围 | 把上面同一段代码改成 `tables.reverse()` 后再迭代 | 无——**已知不被任何断言守护，且大概率永远不会被守护**：phase 3 对每张表的逐行删减是相互独立的贪心搜索，每个候选删除只用 `still_fails` 单独判定是否保留，不依赖其他表当时被缩到什么程度；实测反转顺序后 `cargo test --workspace --locked --no-fail-fast` 全绿（12/12 集成测试仍通过）。这与"只处理第一张表"是两类不同的缺口：后者是覆盖率缺口（有表整个没被访问到），前者是顺序敏感性缺口——而这个算法结构下顺序客观上不影响结果，不是测试没写到 | 不适用 |
| §9.1 `check_batch_invariance` 必须能在真正的多表用例上跑通（I4，最终评审发现：探针同上，证实此前没有任何调用点喂给它一个真正的多表用例） | 新增 `batch_invariance_holds_for_naive_engine_on_a_two_table_case`，用两表 `Database` 跑 `check_batch_invariance` | `batch_invariance_holds_for_naive_engine_on_a_two_table_case`（这条只钉死"多表用例能跑通 `check_batch_invariance` 而不 panic/不报错"；与 I2/上面两行 §8.5、§8.2 缺口同一个根因，`materialize()` 与 oracle 都只读 anchor 表，非 anchor 表的 delta 是否真的影响了批次无关性的结果，在 Phase 1 里无法被任何断言区分——这一层留到 join 落地后重新处理，见文末"Join 落地"清单） | 不适用——**本行从未跑过变异**：这一行登记的是「新增了一个多表测试」，而不是「改坏什么会让它变红」。实测确认它抓不到非 anchor 表的簿记错误（让 `NaiveRecompute::apply` 丢弃非 anchor delta 后全套仍 113/113 绿），根因与上面两行相同：Phase 1 的 oracle 只渲染 anchor 表。join 落地后按文末清单第 3 条重新处理 |

## ivmlite-workload / ivmlite-bench

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §10.1 分组键数量精确等于 `group_cardinality` | 改为纯随机分配 | `first_card_rows_deterministically_cover_each_group_in_order`（缺口修复新增；原 `rows_respect_group_cardinality` 在 500 行/7 键下纯随机也几乎必然覆盖全部键，抓不住这个变异，细节见 task-1-report.md） | **已验证** |
| §10.3 #7 `card > base_rows` 须拒绝而非钳制 | 去掉 `validate` 中的比较 | `load_rejects_group_cardinality_exceeding_base_rows` | **已验证** |
| §10.3 #7 边界 `card == base_rows` 须接受 | 比较改为 `>=` | `base_rows_equal_to_group_cardinality_is_accepted` | **已验证** |
| §10.3 #7 格子推导属 workload 而非 runner | 改视图阈值公式的 `*` 为 `+` | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证** |
| §10.3 #7 同上：跳过规则 | 删掉 `cells()` 里的跳过判断 | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证** |
| §10.3 #7 同上：两次扫描结构 | 改为四维全交叉 | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证**（实现者） |
| §10.3 #4 bootstrap 须先于 trigger 创建 | 删掉 `INSERT ... SELECT` | `trigger_maintained_table_matches_direct_query_after_seed_and_updates` | **已验证** |
| §11 基线曲线须可读 | y 轴改回线性 | `y_axis_uses_log_scale_not_linear` | **已验证** |
| §10.3 #7 `variant` 不得绕过校验 | 去掉 `variant()` 里的 `validate()` 调用 | `variant_rejects_cardinality_exceeding_base_rows` | **已验证** |

---

## 统计与欠账

表内共 **113** 行：已验证 **96** 条、未验证 **0** 条、不适用 **17** 条
（`Value` 无 Real/Blob 由类型系统而非测试守护，加变体会编译失败）。

这三个数字由 `scripts/count-mutation-gates.py` 从本文件数出来，不是手写的——
手写的第一版就错了（写成 12/26），而这恰好是本表存在的理由的一个小样本：
**声称与事实之间的缝隙，不靠更用心去写来关闭，靠让它可被机械核对来关闭。**

这条教训还得再吃一次才算学会：脚本当初没进仓库，于是"数字是脚本数的"
本身变成了一处无法复核的声称。M1a Task 3 加了两行、统计仍停在 39/38，
直到脚本补进仓库、第一次运行就报出不一致。加行时请跑：

```bash
python3 scripts/count-mutation-gates.py --fix
```

**这个脚本不检查什么**（2026-09-21 补，起因见下）：它只核对文末统计句与表格
单元格的字面内容是否一致。它**不**判断某一行的"已验证"是否真的跑过变异，也
**不**核对表格与本文档其他散文段落是否自洽。M1a Phase 1 最终评审的定向复审
就抓到过一次：`batch_invariance_holds_for_naive_engine_on_a_two_table_case`
那一行被标成"已验证"，而它的"变异"格描述的是*新增一个测试*而不是改坏什么，
实际从未跑过变异；文末"Join 落地"清单三行之后还明写这三条"标记都是不适用"。
脚本照样报"一致"。

所以：**`一致` 不等于这张表是诚实的**，它只等于数字没抄错。一行的"已验证"
是否名副其实，仍然只能靠真的去跑那条变异——这正是本文开头那段话的意思，
而它连这份文档自己都没能豁免。

M1a Phase 1 Task 1（2026-09-20）把此前标"未验证"的 25 条逐条真的改坏、跑了一遍
`cargo test --workspace --locked`：23 条按原表所记的测试变红；2 条不是——
一条什么都没红，一条红的是另一个巧合命中的测试，两条都不是原表登记的那个
"会红的测试"真的守住了不变量。这正是本表开头那段话预告的：十三轮阅读式
评审找到零个这类缺口，机械跑一遍变异就找到了。两条缺口都补了新测试并跑通
了同样的"改坏→编译→测试→还原"流程，细节与逐行记录见
`task-1-report.md`。

**seed↔case 对应关系在 M1a Phase 1 被重新基准化（m2，最终评审记录）。**
§9.4 的前提是"一个 seed 命名一个用例"，但单表 wrapper 在多表化之后
（`ops.rs` 的 `gen_ops` 里 `rng.random_range(0..1usize)` 那次选表）每步
都会多消费一次 RNG 抽取，即使 `db` 只有一张表也一样。结果是：同一个
`seed`，`gen_case(seed, …)` 在 M1a Phase 1 之后产出的用例，与 M0 版本对
同一个 `seed` 产出的用例不再相同。没有任何东西坏了——检出率与收敛步数
两条验收阈值都不受影响，`tests/regressions/` 下的 fixture 是按格式迁移
（数据本身照搬，只改了容器形状）而不是重新生成，所以不存在"回归用例
悄悄换了一个"的风险。但如果有人拿着 M0 时代写下的"seed N 能复现 X"这类
笔记来重放，得到的会是别的用例——这件事在此之前没有记在任何地方。以后
若要引用某个 seed 复现某个失败，请注明是 M1a Phase 1 之后的版本。

## M1 的登记要求

M1 新增的每一条 spec 强制行为——delta consolidation、bootstrap 水位原子性（§7.3）、delta GC（§7.2）、算子的增量规则（§6.1 三类）、控制面的销毁顺序（§8.3，方案 A 的孤儿 trigger 会毒死基表）——都必须在此登记一行，且"已验证"一栏必须是真跑过的。

计划里每写一条"必须满足 X"，就要同时写出"若 X 被删会红的那个测试"，并在这张表里占一行。

**Join 落地（引擎计划 Phase 3）时必须重新处理的六条**（最终评审 Finding F
重新点数、重新分类；此前这里写的是"四条"，实际列着五个编号项，且第五项
（`BTreeMap`/`HashMap` 排序那条）从未被下面的收尾段落提到过——见本节末尾
的更正说明），标记都是"不适用"而不是"已验证"，原因分三类：前四条是
Phase 1/Phase 2 的 oracle 只渲染 anchor 表的单表 SQL，非 anchor 表的状态
天生不可观察；第五条是 M1a Phase 2 Task 2 落地 `Arrangement`/
`MemArrangement` 时根本没有消费者，集成层面无处下手做变异；第六条是
M1a Phase 2 Task 6 的合并顺序在当前 `ZSet::merge` 语义（逐点加法、与调用
顺序无关）下不可观察。

1. `§8.5` 表格里"让 `NaiveRecompute::apply` 静默丢弃非 anchor 表"——引擎侧。
2. `§8.2` 表格里"让 `run` 自己的 `bases` bookkeeping 跳过非 anchor 表"——
   harness 侧（I2，最终评审新增）。
3. `§9.1` 表格里 `batch_invariance_holds_for_naive_engine_on_a_two_table_case`
   ——这条测试目前只证明多表用例能跑通 `check_batch_invariance` 而不出错，
   不证明非 anchor 表的 delta 真的参与了比对（I4，最终评审新增）。
4. `ivmlite-core` 表格里 `IncrementalEngine::create_view` 的 bootstrap 循环
   只处理 anchor 表那一行（M1a Phase 2 Task 5 新增；最终评审 Finding F 补记：
   此前这一行自己的"已验证"列写着"并入文末「Join 落地」清单，第 1-3 条在
   引擎侧的第四个同构对应物"，但清单里从来没有真的列出这第四项——引用
   悬空了整整一个版本。这里补上，让引用有地方落）——与前三条同一个根因：
   查询与 oracle 都只渲染 anchor 表的单表 SQL，非 anchor 表的初始状态
   在 `materialize()` 和 oracle 比对里天生不可观察。**这一条内部还要再分
   两半**（最终评审 Finding E）：「声明了表却没给初始状态必须报错」这条性质
   现在已经被 `create_view_errors_when_a_declared_table_has_no_initial_state`
   挡住、算已验证；「非 anchor 表的初始行真的被 bootstrap 正确吸收（哪怕
   全部表都给了初始状态）」这条性质仍然不可观察，仍然待 join 落地重新处理
   ——不要把两者混着看成同一件事的"不适用/已验证"。
5. `ivmlite-core` 表格里 `MemArrangement` 的那一行（M1a Phase 2 Task 2 新增，
   复审 Finding 5 之后拆成 5a/5b——两类不能混在一起处理）
   ——join 是 `Arrangement` 在这份计划里的第一个真实消费者：v0 的 `Aggregate`
   用普通 `BTreeMap` 存 group 状态，从不经过 `Arrangement`，所以 Task 2 登记
   的 5 条"已验证"变异目前只被 `MemArrangement` 自己的单元测试守着，没有
   任何集成路径能验证 join 算子真的按 `Arrangement` 的契约在用它。

   - **5a（公开 trait 契约，可移植、可重新验证）**：`get` 只靠迭代 key 的多个
     值（`one_key_can_hold_multiple_values`）、归零删除（`weights_accumulate_
     and_zero_removes_the_entry` / `a_key_with_no_values_left_disappears_
     from_scan` 的 `scan()` 断言部分）、`scan()` 顺序确定（`scan_order_is_
     deterministic`）、`scan()` 输出不依赖 update 历史（`scan_order_is_
     independent_of_update_history`）——这 4 条只通过 `Arrangement` 的公开
     方法（`get`/`update`/`scan`）观察，原则上可以对**任何** `Arrangement`
     实现重跑，包括 join 里真正用到的那个实现。join 落地时必须把这 4 条
     变异原样重跑一遍，确认它们在有真实消费者之后仍然会红。
   - **5b（`MemArrangement` 私有实现细节，不可移植、不可重新验证）**：只删
     `if vals.is_empty() { self.inner.remove(key); }` 一行那条变异，`会红的
     测试` 那一列已经记录得很清楚——它靠的是直接查 `MemArrangement` 私有
     字段 `inner` 的白盒断言（`a.inner.contains_key(...)`），而不是任何公开
     方法的输出。这条断言天生绑死在 `MemArrangement` 这一个类型上，join
     落地后无论怎么跑，都不可能通过 `dyn Arrangement` 或任何别的实现重新
     验证——不是"暂时没测到"，是这条测试的写法本身就只能测这一个实现。
     M1b 的 SQLite shadow table 实现不继承这条守护：它必须自己判断"清空
     后是否会留下僵尸状态"这件事在 `DELETE`-based 实现里是否存在（很可能
     不存在——SQL `DELETE` 没有"空壳容器"这个概念），如果存在就自己写一条
     等价的测试，不能因为 `MemArrangement` 这边"已验证"过就默认它也没事。
6. `ivmlite-core` 表格里 `refresh` 合并按表分组用 `BTreeMap` 换 `HashMap`
   那一行（M1a Phase 2 Task 6 新增）——单表时 `by_table` 只有一个 key，顺序
   无意义；多表时当前 `for (table, delta) in &by_table` 的推进顺序只影响
   `self.view.merge(...)` 调用的先后，而 `ZSet::merge` 是逐点加法、与调用
   顺序无关，所以现在没有任何观察点能看出这个顺序，实测也确认了这一点
   （163/163 全绿）。join 落地后 `ΔR⋈ΔS` 项会同时读两侧 arrangement 的
   当前状态，两侧 `apply`/`refresh` 的先后顺序第一次会影响输出——那时必须
   把这条变异重新跑一遍，确认它转红，再把"不适用"改成"已验证"。

**第二条比第一条更要命**，这也是它被单独列出来的原因：`bases` 不是某个
待测引擎的内部状态，它是直接喂给 `recompute_via_sqlite` 的 oracle 输入。
如果只重新验证第一条（引擎侧）而漏了第二条，join 落地后会出现这样的
局面——harness 能把 delta 正确路由给引擎（`apply` 收到的表名和 raw delta
都对），但喂给 oracle 的 `bases` 映射里非 anchor 表却冻结在 bootstrap 时的
状态；oracle 于是拿一个过期的 `S` 去算 `ΔR⋈S`，得到一个同样错误的
`want`。这不是"引擎错了、被漏判"，而是**比对的两边一起错、且错得一样**：
`got == want` 会照样成立，`run` 会照样返回 `Ok`，而这正是全套测试里唯一
一个"oracle 自己说谎"却没有任何机制能拆穿的位置。

**到那时必须把第 1、2、3、4 条与第 5a 条、第 6 条都重新跑一遍变异**（最终
评审 Finding F：这句收尾指令此前只列了"第 1、2、3 条与第 4a 条"，既没有
第四条（当时还是悬空引用，见上面第 4 条的补记），也没有第五条（当时的
编号，现在的第 6 条）——尽管第 6 条自己那段说明里明明白白写着"那时必须
把这条变异重新跑一遍"。这句是"不靠任何人记住"的操作性指令，遗漏了就等于
没写），逐条确认它们这次真的会让测试变红，变红后再把对应行的"不适用"改成
"已验证"。第二条尤其不能省——它检查的不是某个待测组件是否正确，而是评判
组件本身是否还站得住。第 5a 条同样不能用"trait 本身的单元测试已经绿过"来
顶替——单元测试证明的是 `MemArrangement` 自己实现对不对，证明不了 join
算子有没有正确地依赖这个 trait（比如误把状态存进自己的局部变量、绕开
`Arrangement` 接口）。第 6 条不能用"单表时无所谓"来跳过——join 落地后
单表用例不再是唯一的用例形状。

**第 5b 条不进入这次重新验证**，原因见上面 5b 自己的说明：它测的是
`MemArrangement` 的私有字段，重新跑变异也不会因为 join 用上了 `Arrangement`
就变得可验证——这条本身就不该、也不能被"重新验证"这个动作覆盖到。它的
"不适用"会一直是"不适用"，直到 M1b 决定 SQLite shadow table 实现是否需要
一条自己的等价测试；如果需要，那是 M1b 自己任务里的新一行，不是把这一行
的状态改掉。这个收尾清单写在这份文档里，不靠任何人记住。
