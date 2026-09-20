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

---

## ivmlite-core

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §5.1 权重归零的行必须删除，不留僵尸行 | 让 `ZSet::update` 在归零时保留条目 | `zero_weight_rows_are_removed_not_kept` | 未验证 |
| §5.1 负权重在中间 delta 中合法 | 让 `update` 钳制负权重 | `negative_weights_are_representable` | 未验证 |
| §9.4 迭代顺序确定（失败用例须可凭 seed 重放） | `BTreeMap` 换 `HashMap` | `iteration_order_is_deterministic` | 未验证 |
| §5.1 `Value` 无 Real/Blob（浮点结合律 / 整数溢出顺序依赖） | 加 `Real` 变体 | 编译失败（类型系统即门禁） | — |

## ivmlite-test：生成器

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §9.2 值域必须窄（否则测不到同组反复增删） | `Domain::default` 的 `distinct` 改大 | `domain_is_narrow_by_default` | 未验证 |
| §9.2 NULL 必须高频 | `null_rate` 默认值改动 | `domain_is_narrow_by_default`（含精确断言） | 未验证 |
| §6.1 整数值域须远离溢出 | 放宽 `Domain` 上界 | `domain_cannot_overflow_integer_sum` | 未验证 |
| §9.2 `distinct == 0` 不得 panic | 去掉 `.max(1)` | `zero_distinct_domain_does_not_panic` | **已验证** |
| §9.2 有偏采样：DELETE 必须命中存在的行 | `gen_ops` 的删改目标改为随机生成 | `deletes_target_rows_that_actually_exist` | 未验证 |
| §9.2 live 集合为空时只能插入 | 去掉 `live.is_empty()` 守卫 | `empty_live_set_only_ever_produces_an_insert_first` | **已验证** |
| §5.2 根算子必须是聚合、group_by 非空 | `enumerate` 产出无聚合或空 group_by 的 query | `enumerate_covers_the_v0_space_and_is_nonempty` | 未验证 |
| §6.1 `SUM` 只作用于整数列 | 让 `enumerate` 对 Text 列产出 `Sum` | `enumerate_only_sums_integer_columns` | 未验证 |
| §6.1 `COUNT(*)` 不带列 | 让 `enumerate` 产出 `Count` 带 `Some(_)` | `enumerate_always_count_has_none` | 未验证 |
| §6.1 谓词白名单三种形式均须被枚举 | 删掉 `IsNotNull` 的生成循环 | `enumerate_covers_the_v0_space_and_is_nonempty` | **已验证**（最终评审） |

## ivmlite-test：语义契约

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §7.1 建表必须 `STRICT` | 去掉 `STRICT` 后缀 | `create_table_sql_is_strict` | 未验证 |
| §6.1 `SUM` 无非 NULL 输入时返回 NULL 而非 0 | 发射分支改看 `total == 0` | `sum_that_totals_zero_is_int_zero_not_null` | 未验证 |
| §6.1 三值逻辑：NULL 谓词不入结果 | `passes()` 对 NULL 返回 true | `is_not_null_predicate_filters_out_null_rows` | 未验证 |
| §5.1 权重 ≤ 0 的行不参与重算 | 删掉 `weight <= 0` 守卫 | `retracting_a_row_that_was_never_inserted_is_a_noop` | 未验证 |
| §9.1 oracle 须按权重展开行 | 每行只插一次 | `expands_rows_by_weight` | 未验证 |
| §9.1 基表状态出现负权重须报错而非静默 | 改为跳过负权重行 | `rejects_negative_weights_in_base_state` | 未验证 |
| §9.1 输出行宽须等于 `output_arity()` | 删掉宽度检查分支 | `rejects_wrong_row_width` | 未验证 |
| §9.1 group key 不得重复 | 删掉重复检查 | `rejects_duplicate_group_keys` | 未验证 |

## ivmlite-test：驱动与接缝

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §8.5 `apply` 收未合并的原始 Δ | 在 `batches()` 里重新折叠进 `ZSet` | `apply_receives_unconsolidated_raw_deltas` | **已验证** |
| §8.5 `apply` 带表名 | 传死值而非 `schema.table` | `apply_receives_the_schema_table_name` | 未验证 |
| §9.1 bootstrap 之后立即比对 oracle | 删掉 `compare(engine, &base, "bootstrap")` | `bootstrap_drift_is_caught_at_the_bootstrap_stage` | **已验证**（最终评审） |
| §9.1 每个 refresh 点都比对 oracle（非仅末尾） | 只在循环结束后比对一次 | `per_batch_oracle_comparison_catches_transient_drift` | 未验证 |
| §9.3 shrinker 的合法性门禁 | `is_legal` 函数体替换为 `true` | `dangling_delete_is_illegal` / `dangling_update_is_illegal` / `delete_after_insert_of_a_different_row_is_illegal` | **已验证** |
| §9.4 `IVMLITE_SEED` 非数字须 panic | 改为静默忽略 | `parse_seed_arg_panics_on_non_numeric_value` | 未验证 |
| §9.1 植入 bug 的引擎必须保持有 bug 且能被抓到 | 修好 `NoRetractionEngine` | `harness_catches_the_missing_retraction_bug` | 未验证 |
| §9.1 漂移污染必须绕过不变量层 | 污染改为只在末列是 `Int` 时生效 | `drift_still_happens_when_the_aggregate_column_is_null` | **已验证** |

## ivmlite-workload / ivmlite-bench

| spec 要求 | 变异 | 会红的测试 | 已验证 |
|---|---|---|---|
| §10.1 分组键数量精确等于 `group_cardinality` | 改为纯随机分配 | `rows_respect_group_cardinality` | 未验证 |
| §10.3 #7 `card > base_rows` 须拒绝而非钳制 | 去掉 `validate` 中的比较 | `load_rejects_group_cardinality_exceeding_base_rows` | 未验证 |
| §10.3 #7 边界 `card == base_rows` 须接受 | 比较改为 `>=` | `base_rows_equal_to_group_cardinality_is_accepted` | 未验证 |
| §10.3 #7 格子推导属 workload 而非 runner | 改视图阈值公式的 `*` 为 `+` | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证** |
| §10.3 #7 同上：跳过规则 | 删掉 `cells()` 里的跳过判断 | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证** |
| §10.3 #7 同上：两次扫描结构 | 改为四维全交叉 | `cells_reproduce_exactly_the_published_csv_matrix` | **已验证**（实现者） |
| §10.3 #4 bootstrap 须先于 trigger 创建 | 删掉 `INSERT ... SELECT` | `trigger_maintained_table_matches_direct_query_after_seed_and_updates` | **已验证** |
| §11 基线曲线须可读 | y 轴改回线性 | `y_axis_uses_log_scale_not_linear` | **已验证** |
| §10.3 #7 `variant` 不得绕过校验 | 去掉 `variant()` 里的 `validate()` 调用 | `variant_rejects_cardinality_exceeding_base_rows` | **已验证** |

---

## 统计与欠账

已验证 **12** 条，未验证 **26** 条。

未验证不等于无效——它们都有具名的守护测试，只是没人真的把实现改坏跑过一遍。但 M0 的经验正是"看起来有守护"和"真的守得住"之间存在系统性差距：最终评审找到的三处缺口，每一处当时都有一个看似相关的测试在。

**M1 开工前应把未验证的 26 条逐条跑一遍。** 按每条约一秒估，不到一分钟，且几乎必然会暴露至少一处"测试在、但抓不住"。

## M1 的登记要求

M1 新增的每一条 spec 强制行为——delta consolidation、bootstrap 水位原子性（§7.3）、delta GC（§7.2）、算子的增量规则（§6.1 三类）、控制面的销毁顺序（§8.3，方案 A 的孤儿 trigger 会毒死基表）——都必须在此登记一行，且"已验证"一栏必须是真跑过的。

计划里每写一条"必须满足 X"，就要同时写出"若 X 被删会红的那个测试"，并在这张表里占一行。
