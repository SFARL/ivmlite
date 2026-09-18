# M-1 Spike：SQLite 扩展机制

- **日期**: 2026-09-18
- **类型**: spike —— **产出是结论，不是要保留的代码**
- **阻塞**: M1（不阻塞 M0）
- **相关**: [总体设计](../superpowers/specs/2026-09-18-ivmlite-design.md) §7、§8.3

---

## 要回答的问题

ivmlite 的控制面需要在扩展内部做两类事：**DDL**（建影子表、建 trigger）和**写入**（更新 arrangement 与输出表）。

问题是：**这两类操作在 SQLite 扩展里，从哪个上下文发起才是安全的？**

早期草案假定可以从标量 UDF 内部做：

```sql
SELECT ivm_create_view('revenue', 'SELECT ...');
SELECT ivm_refresh('revenue');
```

即在一条正在 `sqlite3_step()` 的 `SELECT` 内，用同一个连接做 DDL 和写入。SQLite 对 hook 的重入限制很严——commit/update hook 明确禁止在回调中再操作触发它的连接；application-defined function 的限制虽然宽一些，但仍然是在运行中的语句里递归使用同一连接。

**这不是可以靠"理论上应该可以"通过的地方。** 如果控制面不干净，现在换比 core 写完再换便宜几个数量级。

### 已有的初步信号（不足以定案）

在宿主语言的 sqlite3 绑定里做过一次粗试：UDF 内部 `CREATE TABLE` 与 `INSERT` 都没有报错。但这**不能清账**：

- 测的是宿主绑定的语句处理，不是真实 cdylib 扩展路径
- 只覆盖了最顺的一条路径，没有 rollback / 嵌套事务 / WAL / 双连接
- "写正在被扫描的表"这一项本身就在未定义行为的边缘

同时确认了一个更有价值的事实：**FTS5 已经解决了同形状的问题**。

```sql
CREATE VIRTUAL TABLE docs USING fts5(body);
-- xCreate 建出 docs_data, docs_idx, docs_content, docs_docsize, docs_config
INSERT INTO docs(docs) VALUES ('rebuild');   -- 命令通道
```

所以这个 spike 不是"UDF 行不行"的二选一，而是**两套方案的对照验证，其中方案 B 是主候选**。

---

## 两套方案

### 方案 A：标量 UDF

```sql
SELECT ivm_create_view('revenue', 'SELECT region, SUM(amount) FROM orders GROUP BY region');
SELECT ivm_refresh('revenue');
```

### 方案 B：虚表 + 命令通道（FTS5 惯用法，主候选）

```sql
CREATE VIRTUAL TABLE revenue USING ivm(
    'SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region'
);
INSERT INTO revenue(revenue) VALUES ('refresh');
SELECT * FROM revenue;
DROP TABLE revenue;
```

方案 B 若可行，在三处优于 A：DDL 发生在 SQLite 为之设计的上下文（xCreate）；视图成为 `sqlite_master` 认识的真实对象；`DROP TABLE` 经 xDestroy 自然清理影子表与 trigger，不需要额外的销毁 API。

---

## 探针怎么做

写一个**一次性的** Rust cdylib，不进 workspace，不进 CI，不求好看。它只需要能被 `.load` 进 stock `sqlite3` CLI，并暴露两套控制面各自最小的实现：建一张影子表、建一个 trigger、往影子表写一行。

然后对**两套方案各跑一遍**下面这张矩阵。

| # | 场景 | 观察什么 |
|---|---|---|
| 1 | 裸调用：建影子表 + 建 trigger + 写一行 | 是否成功；`sqlite_master` 是否如预期 |
| 2 | 在显式 `BEGIN ... COMMIT` 内调用 | 是否成功；提交后是否可见 |
| 3 | 在显式 `BEGIN ... ROLLBACK` 内调用 | **影子表与 trigger 是否一并回滚**，还是留下孤儿对象 |
| 4 | 嵌套：`SAVEPOINT` 内调用后 `ROLLBACK TO` | 同上 |
| 5 | WAL 模式下重跑 1–4 | 行为是否与 rollback journal 模式一致 |
| 6 | 连接 A 调用控制面，同时连接 B 正在读 | 是否阻塞、是否报 `SQLITE_LOCKED` / `SQLITE_BUSY` |
| 7 | 连接 A 调用控制面，同时连接 B 正在写 | 同上 |
| 8 | 在对同一张表的 `SELECT` 扫描过程中调用（方案 A 特有） | 是否触发未定义行为——**这是方案 A 最可能出问题的一格** |
| 9 | `DROP TABLE` / 销毁路径 | 影子表与 trigger 是否被清理干净 |
| 10 | 扩展未加载时打开同一个库并写基表 | trigger 是否仍然记录 delta（§8.1 依赖这条性质） |

场景 10 不是控制面问题，但它是 §8.1「纯 SQL trigger 让未加载扩展的连接也被捕获」这条设计声明的直接验证，顺手一起测。

---

## 决策规则

| 结果 | 决定 |
|---|---|
| B 全绿 | **采用 B**，§8.3 定稿，控制面语法固定为 `CREATE VIRTUAL TABLE ... USING ivm(...)` |
| B 有问题但 A 全绿 | 采用 A，并把 B 的失败原因写进 ADR——它是个反直觉的结论，值得记录 |
| 两套都全绿 | 仍选 B（三处优势成立），A 作为备选记录 |
| **两套都有问题** | **在此停下，不进入 M1。** 重新设计控制面：候选包括「只提供读侧虚表 + 维护完全由宿主应用在自己的写事务里调用」、或放弃 loadable extension 形态 |

场景 3、4 的结果即使不致命也必须记录：**若影子表与 trigger 不随事务回滚**，那么"建视图"这个操作就不是事务性的，§7.3 的 bootstrap 原子性论证要重写。

---

## 完成判定

1. 上述 10 个场景 × 2 套方案的结果表落到 `docs/spikes/2026-09-18-m-1-results.md`
2. §8.3 从「待 M-1 验证」改为结论，控制面语法定稿
3. 若结论推翻了 §7 或 §8 的任何论证，先改 spec 再进 M1
4. 探针代码**明确标注为一次性产物**，不合入 workspace

---

## 与 M0 的关系

M-1 与 M0 相互独立：M0 是纯 Rust 的测试与 benchmark 骨架，`ivmlite-core` / `ivmlite-test` / `ivmlite-workload` 都不碰 SQLite 扩展 API（`ivmlite-test` 只用 rusqlite 当 oracle，`ivmlite-bench` 只用它跑基线）。

但 **M-1 先做**，因为它的结论可能改写 §7 与 §8，而那两节是 M1 的地基。
