# ivmlite 设计文档

- **日期**: 2026-09-18
- **状态**: 已批准，待实施
- **名称**: `ivmlite`（沿用 SQLite 的 `Lite` 拼写；crates.io 上未被占用）

---

## 1. 这是什么

ivmlite 是一个 SQLite 扩展，为 SQLite 提供**增量物化视图**（Incremental View Maintenance）。视图的维护成本与变更量 `Δ` 成正比，而不是与基表规模成正比。

以 Rust 编写，数据模型采用 DBSP 的 Z-set（带权重的多重集）。

### 1.1 目标

1. **学习 Rust 与数据库内部机制**——自己实现算子与状态管理，而不是生成 SQL 交给宿主执行。
2. **建立一套可信的 IVM 正确性验证体系**——这是本项目唯一确定无人做过的部分。
3. **得出一个诚实的性能结论**，包括"不值得做"这个结论。

### 1.2 非目标

- **算法创新**。DBSP 已给出完整理论框架，本项目是工程实现，不试图发明新的增量算法。
- **生产可用性**（M2 之前明确不是）。
- **与 Turso 竞争采纳度**。

### 1.3 为什么在已有实现的情况下仍然做

已知同类实现：

| 项目 | 宿主 | 语言 | 架构 | 状态 |
|---|---|---|---|---|
| Turso | SQLite 兼容（Rust 重写） | Rust | DBSP circuit | experimental，on-disk 格式未稳定 |
| duckDBSP | DuckDB | C++ | DBSP | experimental |
| OpenIVM | DuckDB | C++ | SQL-to-SQL 编译 | 研究原型（SIGMOD 2024） |
| Feldera | 独立引擎 | Rust | DBSP，SQL→Rust 代码生成 | 相对成熟，MIT（enterprise 部分除外） |

本项目不假设自己会胜出。价值在于三点：

1. **学习价值**不因别人做过而降低。
2. **这四个实现全部标注 experimental 且语义未稳定**。一套跨实现的差分测试目前无人做，能同时对四个项目产出价值，是干净的 upstream 贡献入口。
3. **Turso 是同宿主、同理论的实现，天然可以当作交叉验证 oracle。**

---

## 2. 为什么目标是 SQLite 而不是 DuckDB

IVM 要回本，四个前提须同时成立：

| 前提 | DuckDB | SQLite |
|---|---|---|
| 进程长期存活（状态才有地方待、才能摊销） | 经常不成立——开进程、扫 Parquet、查一次、退出 | 成立：app 进程 / 浏览器 tab / agent session |
| 数据在进程存活期间变化 | 经常不成立——Parquet 多为静态快照 | 成立，持续变化（OLTP 的定义） |
| 同一个 query 被反复问 | AI 时代反而更弱：agent 生成的是 ad-hoc query，无法预先声明视图 | 强成立：UI 每次渲染 / 每次 sync 重跑同一批 query |
| 重算相对预算够贵 | 数据大，但 DuckDB 本身极快 | 数据小，但预算是一帧 16ms |

结论：**DuckDB 更热，但 SQLite 是 IVM 真正有活干的地方。** 佐证是 local-first 生态（LiveStore、TanStack DB）正在自行实现增量计算——TanStack DB 甚至用 JS 手写了 differential dataflow。

此外 SQLite 在工程上更适合本项目：C API 稳定且小，从 Rust 绑定是成熟路径（`rusqlite` / `libsqlite3-sys`），没有 DuckDB 那样的 C++ ABI 与 FFI 阻抗问题——后者会把"学 Rust"这个目标直接架空。

---

## 3. 交付形态

**v0 形态：SQLite loadable extension（Rust cdylib）。**

覆盖 server / 桌面 / CLI。明确不覆盖浏览器与 iOS 系统 SQLite（两者都无法加载扩展）。

被否决的替代形态见 §12.2。

---

## 4. 架构

### 4.1 Crate 划分

```
ivmlite/
├── crates/
│   ├── ivmlite-core/     纯 Rust，不依赖 rusqlite / libsqlite3
│   ├── ivmlite-sql/      SQL → plan IR（SQLite dialect + SQLite 语义）
│   ├── ivmlite-sqlite/   cdylib：扩展入口、trigger、shadow table、vtab
│   └── ivmlite-test/     差分测试框架（lib + bin，进 CI）
├── benches/
└── docs/
```

依赖方向单向：`ivmlite-sqlite` → `ivmlite-sql` → `ivmlite-core`。

### 4.2 硬约束

> **`ivmlite-core` 不得依赖 `rusqlite` 或 `libsqlite3-sys`。**

这条约束保证 core 可以在完全没有 SQLite 的情况下单元测试（喂 delta 进去、拿 delta 出来），算子正确性测试不需要起数据库。副产品是以后若要做"SQLite 旁边的库"形态不需要返工，但**不为此提前抽象**。

> **所有 `unsafe` 与 FFI 只允许出现在 `ivmlite-sqlite` 中。**

### 4.3 各 crate 职责

**`ivmlite-core`**
- `Value` / `Row` / `ZSet`（带 i64 权重）
- plan IR
- 算子 trait 与实现
- `Arrangement` trait——按 key 索引的状态抽象，形状按 join 需求定义
- 增量化：plan → dataflow

**`ivmlite-sql`**
- `sqlparser-rs`（SQLite dialect）→ plan IR
- 对着 `Catalog` trait 做名字解析与类型推导（trait 化以便测试中伪造，无需真库）
- **子集外的 query 一律硬报错**，不做静默全量回退（见 §12.5）

**`ivmlite-sqlite`**
- `sqlite3_ivmlite_init` 扩展入口
- `ivm_create_view()` / `ivm_refresh()` 标量函数
- trigger 与 delta 表的 DDL 生成
- shadow table 读写、`Arrangement` 的 SQLite 实现
- `Catalog` 的 `PRAGMA table_info` 实现
- bootstrap（在已有数据的表上建视图时的首次全量计算）

**`ivmlite-test`**
- schema / 数据 / 更新序列 / query 生成器
- oracle 执行器（全量重算、本引擎、Turso）
- 保持序列合法性的 shrinking
- seed 重放

### 4.4 状态归属

**v0：算子状态直接存在 SQLite 表中，不做内存缓存。**

换来的：持久化、崩溃恢复、事务一致性、多连接安全——全部由 SQLite 自身的 WAL 与锁免费提供，一行不用写。状态是可直接 `SELECT` 的普通表，调试完全透明。

代价是慢。但 v0 目标是正确性与架构，且这样做有额外好处：**内存 arrangement 缓存变成 M4 的一个可测量优化**，有 M1 的基线数字能证明它值不值，而不是一开始就假设它值。

---

## 5. 数据模型与 plan IR

### 5.1 Z-set

一行的权重是 `i64`。INSERT = `+1`，DELETE = `-1`，UPDATE = `-1`(OLD) 与 `+1`(NEW) 两条。

状态更新是 Z-set 加法（同一行的权重相加）。

**权重不变量**（在差分测试中断言）：

- 中间 delta 出现负权重是正常的。
- **最终物化状态中不允许出现负权重**——出现即为 bug。
- **权重归零的行必须从状态中删除**，不得留 `w = 0` 的僵尸行，否则 `COUNT(*)` 与内存占用都会漂移。

### 5.2 Plan IR

```rust
enum Plan {
    Scan      { table: TableId, columns: Vec<ColumnId> },
    Filter    { input: Box<Plan>, predicate: Expr },
    Project   { input: Box<Plan>, exprs: Vec<Expr> },
    Aggregate { input: Box<Plan>, group_by: Vec<Expr>, aggs: Vec<AggSpec> },
    Join      { left: Box<Plan>, right: Box<Plan>, on: Vec<(Expr, Expr)> },  // M2
}
```

v0 实现前四个，`Join` 占位但不实现。

### 5.3 视图定义的持久化

**存 SQL 原文，不存序列化的 IR。** 重连时重新 parse。

这样 IR 可以自由演进而无需数据迁移。Turso 目前正卡在"on-disk 格式不稳定、旧版本的视图读不了"这个问题上，是现成的教训。

---

## 6. 算子与增量化

### 6.1 三类算子

| 类别 | 算子 | delta 规则 | 需要状态 |
|---|---|---|---|
| **线性** | Filter, Project | `Δ(f(R)) = f(ΔR)` | 否 |
| **双线性** | Join | `Δ(R⋈S) = ΔR⋈S + R⋈ΔS + ΔR⋈ΔS` | 是，两侧各一个 |
| **聚合** | SUM, COUNT | 组内可增量维护 | 是 |

线性算子是白送的——delta 直接穿过，无状态。**v0 的全部难度集中在聚合。**

MIN/MAX 不属于上述任何一类：删除当前最小值时需要知道次小值，必须另配数据结构。因此排在 M4。

#### 聚合的 NULL 语义契约

**`SUM` 在非 NULL 输入为零行时返回 `NULL`，不是 `0`。** 已实测确认：

```sql
CREATE TABLE u(g TEXT, v INTEGER) STRICT;
INSERT INTO u VALUES ('a', NULL), ('a', NULL);
SELECT g, typeof(SUM(v)), COUNT(*) FROM u GROUP BY g;   -- a|null|2
```

注意这与"组为空"是两种不同情形：组为空时该组根本不出现在输出里；组非空但**该列全为 NULL** 时，组出现，`COUNT(*)` 为正，而 `SUM` 为 `NULL`。

因此 `SUM` 的算子状态必须同时维护 **累加值** 与 **非 NULL 输入的计数**，输出时按后者是否为零决定发 `Int` 还是 `Null`。只维护累加值的实现会在该情形下输出 `0`，与 SQLite 静默不一致。

`COUNT(*)` 不受影响——它计的是行数，与列值是否为 NULL 无关。

### 6.2 聚合的 retraction 语义

**这是 IVM 最大的 bug 来源，必须严格遵守。**

某个 group 的 `SUM` 从 100 变为 150 时，输出的 delta **不是** `+1 行 (region, 150)`，而是：

```
(region, 100)  weight −1     ← 撤回旧的输出行
(region, 150)  weight +1     ← 发出新的输出行
```

聚合算子必须记住**自己上一次对外发出过什么**，才能撤回它。这是聚合需要状态的真正原因。因此 `Aggregate` 的状态中既包含累积量 `(sum, count)`，也包含**当前对外的输出行**。

### 6.3 Arrangement trait

```rust
trait Arrangement {
    fn get(&self, key: &Row) -> Box<dyn Iterator<Item = (Row, i64)> + '_>;
    fn update(&mut self, key: &Row, val: &Row, weight_delta: i64);
    fn scan(&self) -> Box<dyn Iterator<Item = (Row, Row, i64)> + '_>;
}
```

**`get` 返回的是多个值而非 `Option`。** v0 的 group-by 每个 key 只存一个值，用不上多值；但 join 的每一侧都是 key → 多行。

> **实现注记**：此处刻意使用 `Box<dyn Iterator>` 而非 RPITIT（`-> impl Iterator`）。`ivmlite-core` 的算子需要持有由 `ivmlite-sqlite` 提供的 `Arrangement` 实现，若用 RPITIT 则该 trait 不是 object-safe，无法 `dyn Arrangement`，会迫使类型参数在整个算子树上传播。装箱的迭代器在 v0（状态本就走 SQLite 表、每次访问都有 IO）中开销可忽略。若 M4 引入内存 arrangement 后测出装箱成为瓶颈，再改为泛型参数化——届时算子树已稳定，改动可控。

> **约束：v0 不允许做出任何会导致 M2 加入 join 时返工的设计决定。** `Arrangement` 的 key → 多值形状是这条约束的主要落点。

trait 定义在 `ivmlite-core`，实现由 `ivmlite-sqlite` 提供（v0 = shadow table）。

---

## 7. 状态表示（shadow table schema）

```sql
__ivm_view(name TEXT PRIMARY KEY, sql TEXT)              -- 视图定义，存 SQL 原文
__ivm_delta_<table>(seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    w INTEGER, <表的所有列...>)            -- CDC，w 即 Z-set 权重
__ivm_state_<view>_<op>(key BLOB, val BLOB, w INTEGER,
                        PRIMARY KEY(key, val))            -- arrangement
__ivm_out_<view>(<输出列...>, __w INTEGER)                -- 物化输出，普通表
__ivm_progress(view TEXT, tbl TEXT, applied_seq INTEGER)  -- 水位
```

`__ivm_out_<view>` 是**普通表**，不加载扩展也能 `SELECT`。

### 7.1 Row 编码的两个语义陷阱

**陷阱一：SQLite 中 `1 = 1.0` 为真，但 INTEGER 与 REAL 是不同的存储类。** 若编码为不同 BLOB，同一个 SQL 意义上的 group key 会分裂成两组。

> **对策：要求 STRICT table，并且额外显式拒绝 `ANY` 列；且 v0 只允许裸列作为 group-by key，不允许表达式**（表达式仍可能产出混合类型）。
>
> **STRICT 本身不足以钉死列类型**——STRICT 表允许 `ANY` 列，该列按原样存储、逐行类型可不同。已实测确认：
>
> ```sql
> CREATE TABLE t(a ANY) STRICT;
> INSERT INTO t VALUES (1), ('1');
> SELECT count(*) FROM (SELECT a FROM t GROUP BY a);  -- 2
> ```
>
> 因此 `ivm_create_view` 必须遍历 `PRAGMA table_info` 的 `type` 字段，遇到 `ANY` 直接拒绝。v0 接受的列类型白名单为 `INTEGER` 与 `TEXT`（`REAL` 因浮点结合律排除，`BLOB` 排在 M4）。

**陷阱二：Collation。** `GROUP BY name` 若列上有 `COLLATE NOCASE`，编码不遵守就会与 SQLite 的分组结果不一致。

> **对策：v0 只支持 BINARY collation，其余一律在 `ivm_create_view` 时拒绝。**
>
> **`PRAGMA table_info` 读不到 collation**——它只返回 `cid, name, type, notnull, dflt_value, pk`，没有 collation 字段（已实测确认）。列的声明 collation 只能从 DDL 本身获得。因此检测办法是：
>
> ```sql
> SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?;
> ```
>
> 取回建表语句，若其中出现 `COLLATE`（大小写不敏感匹配）则拒绝该表。这是保守的过度拒绝——`COLLATE` 可能出现在与 group-by 列无关的位置——但 v0 宁可误拒也不能误纳：漏掉一个 NOCASE 列会让物化结果与 SQLite 静默不一致，而差分测试未必覆盖得到用户的真实 collation 配置。精确到列的判断排在 M3。

编码必须是**规范的**：同一逻辑行必须始终编码为完全相同的字节序列。

---

## 8. CDC 与维护时机

### 8.1 Trigger（纯 SQL，不调 UDF）

```sql
CREATE TRIGGER __ivm_orders_ins AFTER INSERT ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (+1, NEW.region, NEW.amount);
END;

CREATE TRIGGER __ivm_orders_del AFTER DELETE ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (-1, OLD.region, OLD.amount);
END;

CREATE TRIGGER __ivm_orders_upd AFTER UPDATE ON orders BEGIN
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (-1, OLD.region, OLD.amount);
  INSERT INTO __ivm_delta_orders(w, region, amount) VALUES (+1, NEW.region, NEW.amount);
END;
```

UPDATE 拆成 retract + insert，**写入 delta 表的内容本身已经是 Z-set**，无需额外转换。

**纯 SQL trigger 的关键性质：即使某个进程没有加载本扩展，它的写入照样被捕获**，delta 表继续累积，下一个加载了扩展的连接能追上。用 hook 做不到这一点。

v0 捕获表的**所有列**，不做按需裁剪。宽表上有浪费，但避免了"新增视图需要变更 delta 表结构"。

### 8.2 维护时机：v0 采用显式 refresh

```sql
SELECT ivm_refresh('revenue');
```

两条自动化路径均不适合 v0：

- **vtab 读时自动 drain**：SQLite 的读事务不能写，做不到。
- **trigger 内调 UDF 立即维护**：技术上可行（已在写事务内，不构成重入），但 **SQLite 只有行级 trigger，没有语句级**——插入 1 万行会触发 1 万次维护，彻底破坏批量 delta 优化，bulk load 不可用。

**显式 refresh 不只是妥协，它是测试矩阵成立的前提**，见 §9.1 的批次无关性。

自动 drain 的 vtab 排在 M3。

---

## 9. 验证体系

核心断言：

```
recompute(Q, D_final) == materialize(IVM(Q, Δ₁..Δₙ))
其中 D_final = D₀ + Δ₁ + ... + Δₙ
```

v0 无浮点，因此是**严格相等**（浮点 SUM 不满足结合律，增量与全量重算不会 bit-for-bit 相等——这是数学性质不是 bug，处理策略排在 M4）。

### 9.1 四层验证

| 层 | 内容 | 需要 oracle |
|---|---|---|
| **不变量** | 最终状态无负权重；无 `w=0` 僵尸行；`applied_seq` 单调且不超过 delta 表水位；每个 group key 在输出表中恰好一行 | 否 |
| **批次无关性** | 同一串 delta，一次性应用 / 逐条应用 / 随机分批 → 最终状态必须完全一致 | 否 |
| **主 oracle** | 同连接内全量重算，**在每一个可观察的 refresh 点**严格比对，而非只比对最终状态 | 权威 |
| **交叉验证** | Turso MV 跑同样的 query 与更新序列 | M2+，不一致说明至少一方有 bug |

批次无关性是抓状态污染类 bug 最有效的断言之一，而自动维护模式下无法测试它。

**为什么 oracle 必须逐批比对而非只比最终状态**：只在末尾比对时，一个"中途算错、形式上仍合法、后续又自行恢复"的实现可以完全通过——而这正是状态漂移类 bug 的典型形态（某个算子的累加器偏了，直到下一次该组被整体重写才被抹平）。不变量层拦不住它，因为错误的值同样满足"权重为 1、group key 唯一"。代价是测试复杂度从 O(n) 变成 O(n × 基表规模)，因此**差分测试的用例规模必须保持很小**（默认 25 行初始数据、150 步操作），大规模场景交给 benchmark 而非正确性测试。

### 9.2 生成器的三个关键设计

**1. 值域必须故意收窄。** 若 `region` 有 100 万个不同值，每个 group 只有一行，则永远测不到"同一个 group 反复增删"——而那正是 retraction 与僵尸行 bug 的产地。值域压到 5–20 个不同值，逼迫碰撞高频发生。NULL 也需高频出现（NULL 在 GROUP BY 中自成一组，是经典 bug 点）。

**2. 更新序列必须有偏采样。** 纯随机生成器在 IVM 测试中几乎抓不到 bug——随机 DELETE 很少命中真实存在的行。生成器需要相当比例的操作**从当前表中采样已存在的行**来删除/修改，并能刻意制造两种序列：
- 删掉刚插入的行（权重归零路径）
- 把一个 group 删空再填回来（retraction + 僵尸行温床）

**3. v0 的 query 空间小到可以穷举。** 列子集 × 谓词 × group-by 列 × 聚合函数，v0 范围内组合数有限——**穷举优于随机**，可复现且覆盖完全。随机性留给更新序列。

### 9.3 Shrinking

**必须自研，不能直接用 `proptest`。** 朴素的序列缩小会产生**非法序列**（删掉一个 INSERT，后续针对该行的 DELETE 就悬空了）。需要一个保持序列合法性的 delta-debugging 缩小器：先缩更新序列，再缩 query，再缩数据。

没有 shrinking，失败时面对的是数千步序列，无法调试。

### 9.4 可复现性

所有随机走 seed；失败时打印 seed，可一条命令重放；失败用例固化进 `tests/regressions/` 成为永久回归测试。

### 9.5 测试分层

- **L0**：`ivmlite-core` 单元测试，不碰 SQLite，手工构造 Z-set 喂算子
- **L1**：差分测试，单视图，穷举 query × 随机更新序列
- **L2**：多视图、级联视图（M2+）
- **L3**：Turso 交叉验证（M2+）

---

## 10. Benchmark

### 10.1 主 benchmark

```
N 个视图 (N = 1, 10, 50, 200)
  × 基表规模 (10k, 100k, 1M 行)
  × 每批 delta 大小 (1, 10, 100, 1000 行)
→ 测量：应用一批 delta 并使所有视图变为最新所需时间
```

### 10.2 对照组按角色分层

| 角色 | 对照物 | 说明 |
|---|---|---|
| **下界** | 只写基表不维护 | 纯写入成本 |
| **怀疑者** | 手写 trigger 维护的汇总表 | **v0 必须打赢，否则没有故事** |
| **基线** | 朴素重跑 | 交叉点在此测量 |
| **同行** | Turso MV | 同宿主、同 DBSP，最公平 |
| **天花板** | `dbsp` crate 裸跑（手搓 circuit，不过 SQL，不落盘） | 与本项目的差距即为 SQLite / 存储税，诊断价值极高 |
| **参考** | duckDBSP / OpenIVM / pg_ivm | 跨宿主，只作背景不作裁决 |

"怀疑者"这一栏对应的是对 v0 最直接的质疑：单表 GROUP BY + SUM/COUNT 就是人们手写了三十年的 trigger 汇总表。必须正面回答。

### 10.3 方法论约束

1. **跨宿主的绝对耗时不可比**（DuckDB vs SQLite 量的是宿主而非 IVM）。跨宿主只比**同宿主内的加速比** `朴素重跑 / 增量`。
2. **CI 中只保留 same-host 对照组**；跨宿主对比做成一次性 writeup，不进 CI（否则必然腐烂）。
3. **所有对照组必须维护完全相同的视图集合。** 若"手写 trigger"只能表达单一形状而"朴素重跑"跑的是另一批查询，测出的倍数无法用于 §10.4 的结论。视图集合的上限由**表达能力最弱的那个对照组**决定——v0 即 `GROUP BY <单列> → SUM, COUNT`，所有对照组一律用这一形状的 N 份副本。
4. **所有对照组必须在计时开始前完成初始状态构建。** 在基表已有数据之后才创建空的汇总表，得到的是一个从不完整的视图，其维护成本也不具代表性。每条基线都要先完成一次全量 bootstrap，再开始测量增量成本。
5. **测试数据必须有稳定主键，删改按主键定位。** 按全部列的值去找行会退化成全表扫描（`EXPLAIN QUERY PLAN` 显示 `SCAN`），使耗时随基表规模线性增长——而基表规模项正是这条 benchmark 唯一要证明的东西，被扫描淹没后结论归零。

### 10.4 要得出的结论

IVM 耗时应随 **Δ 大小**增长、几乎不随**基表规模**增长；朴素重跑随基表规模线性增长。**真正的结论是交叉点在哪里。**

> **benchmark 的设计必须能够证伪本项目。** 若交叉点落在 100 万行以上，则对典型 SQLite 用户没有意义。这个数字必须敢测、敢认。一个只会得出好结论的 benchmark 没有价值。

### 10.5 次要指标

- **写放大**——安装本扩展后，trigger 使**所有**写入变慢，哪怕从不读视图。这是 IVM 的隐藏税，必须量化，否则收益数字是假的。
- 空间放大：state + delta 表 vs 基表
- bootstrap 耗时

### 10.6 建立时机

**benchmark harness 先用朴素重跑实现填充"待测引擎"的位置**，于是第一天就有完整基线曲线，实现 core 的每一步都有实时对比，而不是做完才知道快慢。

---

## 11. Roadmap

> **顺序约束：测试框架与 benchmark 骨架必须在 core 之前建立。**

### M0 — 测试与基准骨架（core 之前）

- crate 骨架 + CI
- 生成器：schema / 数据 / 有偏更新序列 / query 穷举
- 四层断言
- 保持合法性的 shrinking、seed 重放
- "待测引擎"位置先塞**朴素重跑**（平凡正确）→ 应当全绿
- 再塞一个**故意有 bug 的假实现**（例如聚合不做 retraction）→ 框架必须抓到并缩到最小用例
- benchmark harness + 三条 same-host 基线曲线

> **完成判定：框架能抓到植入的 bug 并 shrink 至 10 步以内；三条基线曲线出图。**

这一步不可省略——不验证"测试框架真的会红"，后续拿到的绿是假绿。

### M1 — v0 引擎

- `ivmlite-core`：Value / Row / ZSet、plan IR、Arrangement trait、Filter / Project / Aggregate(SUM, COUNT)
- `ivmlite-sql`：`sqlparser-rs` → IR，Catalog trait，子集外硬报错
- `ivmlite-sqlite`：cdylib、`ivm_create_view` / `ivm_refresh`、trigger DDL、shadow table、bootstrap

**v0 限制**：STRICT table only；BINARY collation only；group-by key 只能是裸列；无浮点聚合；显式 refresh；单表（无 join）；INSERT / DELETE / UPDATE 全部支持。

> **完成判定：M0 的全部测试绿；交叉点有明确数字；写放大有明确数字。数字难看也算完成。**

### M2 — Join（第一个可辩护的里程碑）

- Join 算子 + 两侧 arrangement
- query 生成器扩展到两表
- Turso 交叉验证接入
- 观察并记录 join 状态爆炸（两侧都需保存全量）

### M3 — 自动维护

- vtab 自动 drain
- 多连接语义

### M4+ — 按价值排序

- 内存 arrangement 缓存（用 M1 的基线证明它值得）
- MIN / MAX（需要额外数据结构）
- 级联视图
- DISTINCT
- 浮点聚合 + tolerance 策略
- OUTER JOIN

### 明确不做（至少一年内）

递归 CTE、窗口函数、correlated subquery、任何分布式能力。

### 项目层面的成功判定

能够说出这句话：**"在 X 行、Y 个视图、Z 更新率下，相对手写 trigger 快/慢 N 倍。"**

**答案是"慢"也算成功**——那是一个真结论。

---

## 12. 决策记录：被否决的方案

### 12.1 为什么不是 DuckDB

见 §2。补充：duckDBSP 已经实现了本项目原计划 v0.1–v0.7 的全部内容（含 DISTINCT、MIN/MAX、窗口函数、递归 CTE、级联视图、持久化），在该宿主上做子集没有意义。且 DuckDB 扩展是 C++，与"学 Rust"的目标冲突。

### 12.2 为什么不是"SQLite 旁边的 Rust 库"形态

该形态（app 写入走自己的 API，类似 LiveStore 的 event-sourcing 模型）天然支持 wasm / 浏览器，能触及 local-first 真实用户。但：拿不到 trigger，CDC 需自行拦截；要求 app 改变写入方式，接入门槛高；benchmark 会混入 wasm↔JS 跨界开销，污染测量。

扩展形态可以直接指向一个已存在的 `.db` 文件，差分测试与 benchmark 都在同一连接内运行，无噪声。这对 M0/M1 的目标更重要。

### 12.3 为什么不用 `preupdate_hook` / session extension 做 CDC

两者均需编译开关（`SQLITE_ENABLE_PREUPDATE_HOOK` / `SQLITE_ENABLE_SESSION`），大量发行版的 stock build 未开启，是实打实的移植税。trigger 到处可用、零开关、随事务回滚，且在扩展未加载时仍能捕获变更。

### 12.4 为什么不走 SQL-to-SQL 编译（OpenIVM 路线）

该路线把视图定义编译成维护用的 SQL 语句，不实现算子、不管状态、不做持久化，join 与聚合直接复用 SQLite 执行器。但：**学到的是编译器而非 dataflow 引擎**，与目标 1 冲突；性能被 SQLite 执行器与 SQL 往返封顶，讲不出"很多视图 × 极小 delta"的低延迟故事；且 OpenIVM 已经做完了。

### 12.5 为什么不做静默全量回退

不支持的 query 在 `ivm_create_view` 时硬报错。v0 阶段的静默降级会掩盖 bug——差分测试会通过，因为全量重算当然等于全量重算。等引擎稳定后再考虑 fallback。

### 12.6 为什么 v0 不建完整的 DBSP circuit

采用**方案 2 的数据模型 + 方案 1 的执行模型**：

- **数据模型从第一天起就是 DBSP 的**——Z-set 带权重、算子是带显式状态的 delta 变换器、状态是按 key 索引的 arrangement。这决定了 join 与递归以后有地方安放，也决定了与 Turso 可比。
- **执行模型先用朴素形态**——没有 circuit scheduler、没有 fixpoint 机制、没有通用 `I`/`D` 算子对。v0 就是"delta 进来，按 plan IR 顺序推一遍，state 更新，结果出去"。
- **join 落地时重新评估是否需要真 circuit；递归上日程时必须上。**

这样 v0 的代码量接近纯手写 delta 规则，但不欠 DBSP 的债。

### 12.7 为什么 DELETE 在 v0 而不是 v0.2

原始计划把 DELETE / UPDATE 放在 v0.2，意味着 v0 是 insert-only——那实质上是个"物化聚合缓存"而非 IVM，且 Z-set 权重、retraction、状态清理这些**全部真难点都会落在"看起来已经做完"之后**。DELETE 是 Z-set 赚钱的地方，必须在 v0。

作为交换，join 从 v0 移出——join 增加的是范围而非架构风险，且 `Arrangement` 的形状已为它预留。

### 12.8 为什么不与 TanStack DB 对比

TanStack DB 是浏览器端 JS 库，与 v0 的扩展形态运行时不同、受众不同。该对比仅在"SQLite 旁边的库"形态下成立（见 §12.2），v0 不采用该形态，故出局。

---

## 13. 已知限制（v0）

1. 仅 STRICT table，且**拒绝 `ANY` 列**（STRICT 本身不排除 `ANY`，见 §7.1）；列类型白名单为 `INTEGER` / `TEXT`
2. 仅 BINARY collation；检测手段是从 `sqlite_master` 取建表语句匹配 `COLLATE`，属保守的过度拒绝（见 §7.1）
3. group-by key 仅支持裸列，不支持表达式
4. 无浮点聚合
5. 无 join
6. 无 MIN / MAX / DISTINCT
7. 需显式调用 `ivm_refresh`
8. delta 表捕获全部列，宽表上有空间浪费
9. 所有写入承担 trigger 写放大，即使从不读视图
10. 无法在浏览器或 iOS 系统 SQLite 上加载
