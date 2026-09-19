# M0 基线基准：结果与结论

`ivmlite-bench`（`crates/ivmlite-bench`）跑三条 same-host 对照组（spec §10.2）：

- `no_maintenance` —— 下界，只写基表，完全不维护任何视图。
- `hand_written_trigger` —— 怀疑者，用手写 SQLite trigger 增量维护汇总表。
- `naive_recompute` —— 基线，每批 delta 之后把全部视图 SQL 重跑一遍（只读，不写回）。

M0 没有真实的 IVM 引擎（那是 M1 的工作），所以这里填的是**三条基线彼此之间**
的关系；下表的结构现在定下来，M1 直接在同一张表上补两列增量系统的数字。

## 本次实际跑的矩阵

跑的是**完整矩阵**，未缩减：

- `BASE_ROWS = [10_000, 100_000, 1_000_000]`
- `BATCH_SIZES = [1, 10, 100, 1000]`
- `VIEW_COUNTS = [1, 10, 50, 200]`
- `GROUP_CARDINALITIES = [10, 1_000, 100_000]`
- 两次扫描（group 基数扫描固定 `views=10`；视图数扫描固定 `cardinality=1000`），
  各基线 32 + 36 = 68 格，三条基线共 204 行数据（`docs/bench/m0-baseline.csv`，
  含表头 205 行）。

全量 release 构建下端到端耗时约 525 秒（约 8.75 分钟），远低于 brief 给出的
20 分钟阈值，因此**没有触发降级到缩小矩阵的分支**——不需要额外说明"跑了什么
而非什么"。

`card=100000 > base_rows=10000` 这一个组合被跳过，对应 **12 行**被省略的 CSV
数据（3 条基线 × 4 个 batch）。这个 guard 只存在于扫描一（group 基数扫描）：
扫描二固定 `cardinality=1000`，对矩阵里任何一个测试过的 `base_rows` 都不会
大于它，所以扫描二永远不会触发这条跳过逻辑。`stderr` 只打了 **3 行**
`跳过无意义格子: card=100000 > base_rows=10000`（每条基线一行，因为
`eprintln!` 在 batch 循环外层，命中一次 `(baseline, card, rows)` 组合就打一
行，而不是每个被省略的 CSV 行都打一行）。这不是漏跑，是
`ivmlite-workload::Workload::load` 在加载时就会拒绝的无意义配置——一张 1 万
行的表容不下 10 万个不同分组键。

**测量方法的一个局限**：每个格子只测了一次，没有热身、没有取多次中位数。
量级结论（10x-24000x 那种差距）不受影响，但最小的格子上能看到轻微的非单调：
`no_maintenance,10,base_rows,1,10` 的 `apply_ms` 依次是 `10,000`→0.006ms、
`100,000`→0.005ms、`1,000,000`→0.017ms——行数更多的一档反而更便宜（第二档
比第一档还低），这在物理上不该发生，说明这些微秒级读数里有噪声，不能把每
一次波动都当成信号来解读。

## Step 5 smoke run 的 sanity check

在缩小矩阵（`BASE_ROWS=[1_000,10_000]`、`VIEW_COUNTS=[1,10]`、
`GROUP_CARDINALITIES=[10,1_000]`）下验证了关键判据：同一 `base_rows` 下，
`naive_recompute.maintain_ms` 应几乎不随 `group_cardinality` 变化（它总要扫
全表），而 `hand_written_trigger.apply_ms` 应随 `group_cardinality` 上升。
`batch=1000, views=10` 处的观测：

| base_rows | naive_recompute.maintain_ms (card=10 → 1000) | hand_written_trigger.apply_ms (card=10 → 1000) |
|---|---|---|
| 1,000  | 2.281 → 2.558 ms（+12%） | 9.227 → 12.610 ms（+37%） |
| 10,000 | 16.533 → 18.956 ms（+15%） | 7.492 → 10.828 ms（+45%） |

`naive_recompute` 几乎不动（个位数百分比，量级由 `base_rows` 决定），
`hand_written_trigger` 明显上升（三成到四成五）——cardinality 维度确实生效，
不是碰巧持平。

同时验证了主键定位不产生 `SCAN`：

```
$ sqlite3 :memory: "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT; EXPLAIN QUERY PLAN DELETE FROM orders WHERE id = 1;"
QUERY PLAN
`--SEARCH orders USING INTEGER PRIMARY KEY (rowid=?)
```

不含 `SCAN`，符合预期。

## 结论表：全量重算 vs 手写 trigger

固定 `views=10`、`base_rows=1,000,000`（矩阵里最大的规模，最能体现"增量成本
不随基表规模增长"这条论点），`Δ 大小` 取 `1` 和 `1000` 两个极端：

| group 基数 | Δ 大小 | 全量重算 / 手写 trigger 比值 | 手写 trigger 的写放大 |
|---|---|---|---|
| 10     | 1 / 1000 | 17647.5 / 292.0 | 7.2x / 12.7x |
| 1k     | 1 / 1000 | 20153.4 / 258.8 | 8.5x / 17.4x |
| 100k   | 1 / 1000 | 24121.2 / 149.7 | 16.7x / 28.7x |

`card > base_rows` 的格子为空——这不是数据缺失，是语义上不存在的配置（N 行的
表不可能有多于 N 个分组键），`ivmlite-workload::Workload::load` 在加载时就会
拒绝这种配置。矩阵里恰好受影响的只有 `base_rows=10,000` 且 `card=100,000` 的
组合。

"比值"= `naive_recompute` 总耗时（apply_ms + maintain_ms）除以
`hand_written_trigger` 总耗时（trigger 成本全部计入 apply_ms，maintain_ms
恒为 0）。"写放大" = `hand_written_trigger.apply_ms` 相对
`no_maintenance.apply_ms` 的倍数——trigger 维护汇总表要多花的那部分写入成本。

## 这张面实际长什么样子（不是一个交叉点）

在整个 204 行的矩阵里，`naive_recompute` 对 `hand_written_trigger` 的总耗时
比值最低点是 **1.42**（`views=1, base_rows=10,000, batch=1000, card=1000`：
naive 2.453ms vs trigger 1.729ms），**没有任何一格出现 `naive_recompute` 更
快**。比值随三个维度移动：

- **base_rows 越大，比值越夸张**：`views=10, card=10, batch=1` 处从
  `base_rows=10,000` 的 133x 涨到 `base_rows=1,000,000` 的 17,647x——
  `naive_recompute` 的成本随基表规模线性增长，`hand_written_trigger` 几乎
  不变（增量成本不随基表规模增长，这正是 benchmark 要证明的东西）。
- **batch 越大，比值越收窄**：同样 `views=10, card=10, base_rows=10,000`，
  batch 从 1 到 1000 时比值从 133x 掉到 2.26x——trigger 每行一次的
  `ON CONFLICT` 更新开始逼近全表扫描一次的成本。
- **views 越少，比值也越收窄**：`base_rows=10,000, batch=1000, card=1,000`
  这一格，视图数从 200 降到 1 时比值从 1.73 一路跌到 1.42——这是全矩阵里最
  接近打平的地方（仍是 `hand_written_trigger` 赢，没有 `naive_recompute`
  反超的格子）。
- **在唯一真正隔离出 cardinality 这个维度的那条扫描上（`views=10` 固定，
  cardinality 在 10/1,000/100,000 间变化），比值是随 cardinality 上升而
  收窄的，不是相反**：`base_rows=1,000,000, batch=1000` 处，比值从
  `card=10` 的 291.97 降到 `card=1,000` 的 258.76、再降到 `card=100,000`
  的 149.71；`batch=100` 处同样从 2428.51 降到 2254.60、再降到 957.22。
  这与"大批量 Δ + 低 group 基数"这个说法里"低基数更窄"的直觉方向相反。
- 上面"views 越少越窄"和"cardinality 越高越窄"是**两条独立的观测，不能相加**：
  本次矩阵是两次独立扫描（一次固定 `views=10` 扫 cardinality，一次固定
  `cardinality=1,000` 扫 views），不是四维全交叉，所以"低 views + 低
  cardinality"这个组合从未被同一次测量同时覆盖过——这份数据对那个角落
  什么都没说，既不能证实也不能证伪。
- **单独说明，不算作 M0 的发现**：`ivmlite` 项目原本的假设是"大批量 Δ + 低
  group 基数"会是 M1 真正的增量引擎（而非这里的手写 trigger）相对全量重算
  优势最大的区域——因为大批量下同一个 group 内的多次修改可以在 delta 消费
  时被合并（delta consolidation），而 trigger 没有这个机制，每行都要单独
  走一次 `ON CONFLICT`。这是留给 M1 去验证的假设，不是这批 M0 基线数据的
  结论；本节前面几条才是这批数据实际支持的内容。

**M0 阶段没有增量引擎可测**，所以"若这张面上不存在任何区域使增量相对全量
重算有实质优势（比值 > 2），则项目前提不成立"这条判据要等 M1 才能真正应用到
"增量系统 vs 全量重算"这一对上。这里能照实报告的是：`hand_written_trigger`
（手写、非通用的增量方案）在全部 204 个测试格子里稳定赢过 `naive_recompute`，
且最窄处比值仍 > 1.4——这是 v0 增量引擎需要打赢的"怀疑者"设下的门槛，M1 接入
真实引擎后要在同一张矩阵上验证它是否也能稳定超过 `naive_recompute`（理想情况
下还要给出优于 `hand_written_trigger` 的故事，因为后者是 M0 不打算打赢、只
用来确认基础设施没坏的下界）。

## 出图

`docs/bench/m0-baseline-card{10,1000,100000}.svg` 各画一张：固定
`views=10, batch=100`，横轴 `base_rows`（对数），纵轴 `apply_ms + maintain_ms`
总耗时，每条基线一条折线。三张图分别对应三个 group 基数——交叉点/差距随
基数剧烈移动，混进一张图会得到一条没有意义的折线（spec §10.1）。

## 原始数据

`docs/bench/m0-baseline.csv`：`baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms`，204 行数据 + 表头。
