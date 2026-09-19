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

`card=100000 > base_rows=10000` 的 8 个格子（2 次扫描 × 4 个 batch）被跳过，
`stderr` 各打了一行 `跳过无意义格子: card=100000 > base_rows=10000`；这不是
漏跑，是 `ivmlite-workload::Workload::load` 在加载时就会拒绝的无意义配置——
一张 1 万行的表容不下 10 万个不同分组键。

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
- **"大批量 Δ + 低 group 基数" 是比值最窄的角落**，与 spec §10.2 三级判据里
  "额外惊喜"唯一可能出现的地方吻合：`base_rows=10,000, batch=1000, card=1,000`
  这一格，视图数从 200 降到 1 时比值从 1.73 一路跌到 1.42——这是全矩阵里最
  接近打平的地方。即便如此，`hand_written_trigger` 仍然赢，不存在
  `naive_recompute` 反超的格子。

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
