# 变异门禁：spec 要求 → 变异 → 会红的测试

## 为什么有这份表

M0 结束时的最终全分支评审用**变异测试**——把实现改坏、看测试会不会红——在约一秒一次的运行里找到三处缺口：shrinker 的合法性门禁（spec §9.3 称之为不用 proptest 的全部理由）打桩成 `true` 后全套仍绿；bootstrap 之后的 oracle 比对整行删掉仍绿；`IsNotNull` 谓词的整个生成循环删掉仍绿。

在那之前，**十三轮基于阅读的逐任务评审一条都没抓到**。这不是评审者不认真——同一批评审在别处抓出了八条实质问题。差别在方法：读代码能判断"这段写得对不对"，判断不了"这段没了会不会有人发现"。

这份表把后一个问题变成机械检查。**每一条 spec 强制的不变量都要有一行**，写明：改坏什么、哪个测试会红。M1 的计划直接要求新增的不变量必须在此登记；评审时对表即可，不必考古。

## 怎么用

- **新增一条 spec 强制的行为时**，同时在此加一行，并真的跑一次变异确认测试会红。
- **"已验证"一栏只填真跑过的。** 推断出来的写"未验证"，不要把推断写成事实——那正是这份表要防的错误类型。
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
| §5.1 `Value` 无 Real/Blob（浮点结合律 / 整数溢出顺序依赖） | 加 `Real` 变体 | 编译失败（类型系统即门禁） | — |
| §5.2 根算子必须是聚合、`GROUP BY` 非空——`ViewQuery` 移进 `ivmlite-core` 正是为了让 M1 引擎直接消费它，边界上却没有任何校验（m6，最终评审发现） | 直接构造 `ViewQuery { group_by: vec![], aggs: vec![], predicate: Predicate::None }`（不经过 `enumerate`），喂给 `check_invariants` | 无——**已知不被现有测试守护**：这条约束今天只在生成器侧成立，因为唯一的生产者 `enumerate` 从不产出这种形状（`enumerate_covers_the_v0_space_and_is_nonempty` 守的是这一点）；但 `ViewQuery` 本身可以在 `enumerate` 之外自由构造，`output_arity()` 对它返回 0，`check_invariants` 对空 `ZSet` 一路放行，不拒绝。实测已验证可构造且通过 `check_invariants`。**不加构造器校验**——引擎计划（Phase 3）第一次让 `ViewQuery` 真正跨越到 `enumerate` 之外的入口（M1 引擎直接消费），边界校验留到那时再补，见文末"Join 落地"清单 | 不适用 |

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

表内共 **56** 行：已验证 **49** 条、未验证 **0** 条、不适用 **7** 条
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

**Join 落地（引擎计划 Phase 3）时必须重新处理的三条**，标记都是"不适用"
而不是"已验证"，原因相同：Phase 1 的 oracle 只渲染 anchor 表的单表 SQL，
非 anchor 表的状态天生不可观察。

1. `§8.5` 表格里"让 `NaiveRecompute::apply` 静默丢弃非 anchor 表"——引擎侧。
2. `§8.2` 表格里"让 `run` 自己的 `bases` bookkeeping 跳过非 anchor 表"——
   harness 侧（I2，最终评审新增）。
3. `§9.1` 表格里 `batch_invariance_holds_for_naive_engine_on_a_two_table_case`
   ——这条测试目前只证明多表用例能跑通 `check_batch_invariance` 而不出错，
   不证明非 anchor 表的 delta 真的参与了比对（I4，最终评审新增）。

**第二条比第一条更要命**，这也是它被单独列出来的原因：`bases` 不是某个
待测引擎的内部状态，它是直接喂给 `recompute_via_sqlite` 的 oracle 输入。
如果只重新验证第一条（引擎侧）而漏了第二条，join 落地后会出现这样的
局面——harness 能把 delta 正确路由给引擎（`apply` 收到的表名和 raw delta
都对），但喂给 oracle 的 `bases` 映射里非 anchor 表却冻结在 bootstrap 时的
状态；oracle 于是拿一个过期的 `S` 去算 `ΔR⋈S`，得到一个同样错误的
`want`。这不是"引擎错了、被漏判"，而是**比对的两边一起错、且错得一样**：
`got == want` 会照样成立，`run` 会照样返回 `Ok`，而这正是全套测试里唯一
一个"oracle 自己说谎"却没有任何机制能拆穿的位置。

**到那时必须把三条都重新跑一遍变异**，逐条确认它们这次真的会让测试变红，
变红后再把对应行的"不适用"改成"已验证"。第二条尤其不能省——它检查的
不是某个待测组件是否正确，而是评判组件本身是否还站得住。这个收尾清单
写在这份文档里，不靠任何人记住。

`§5.2` 根算子约束（m6，见上方"ivmlite-core"表最后一行）同样必须在这次
收尾时一并处理：`ViewQuery` 的合法性目前只在生成器侧（`enumerate`）被
守住，`ivmlite-core` 里的 `check_invariants` 对空 `group_by` / 空 `aggs`
一路放行。join 落地后引擎会直接从 M1 的计划消费 `ViewQuery`，不再只经过
`enumerate` 这一个入口，届时必须补上边界处的校验。
