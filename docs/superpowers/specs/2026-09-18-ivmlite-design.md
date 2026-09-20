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
2. **建立一套可信的 IVM 正确性验证体系**——截至目前**未找到可复用的、跨 IVM 实现的 property-based differential testing harness**。（不宣称"无人做过"：那是无法证明的命题。）
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

#### v0 的根算子必须是带非空 GROUP BY 的 Aggregate

这条约束堵住一个隐蔽的语义漏洞。物化输出表带 `__w` 权重列，但**权重是内部表示，SQL 表没有权重概念**：

```sql
-- 若允许 Scan → Filter → Project 直接成为视图
原始数据: apple, apple, banana
Z-set 表示: (apple, w=2), (banana, w=1)     -- 2 行
用户 SELECT: 应当看到 3 行
```

物化表会显示 2 行而普通 SQL 视图显示 3 行——两者语义不一致，而"物化视图就是一张普通 SQL 表"正是本项目的卖点。

**规定：视图的根算子必须是 `Aggregate`，且 `group_by` 非空。** 于是 group key → 恰好一个输出行，`__w` 在最终输出中恒为 1，Z-set 权重只出现在内部 delta 与算子状态中。

合法：`Scan → Filter → Project → Aggregate`
非法：`Scan → Filter → Project` 直接作为视图

**同时禁止无 GROUP BY 的全局聚合**，因为它与分组聚合的空集行为不同（已实测）：

```sql
SELECT SUM(v) FROM t;            -- 空表 → 1 行（值为 NULL）
SELECT g, SUM(v) FROM t GROUP BY g;  -- 空表 → 0 行
```

"组内计数归零就删掉该行"这条简单规则对前者是错的。与其为一个特例引入第二套规则，不如在 v0 直接拒绝全局聚合。

这两条合起来使 v0 的定位变得精确：**自动维护的聚合**（automatically maintained aggregates），而不是泛化的物化视图。

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

#### 整数溢出：与浮点结合律同类的问题

**SQLite 的 `SUM` 在整数溢出时报错，而且报不报错取决于扫描顺序。** 已实测：

```sql
INSERT INTO o VALUES (9223372036854775807), (9223372036854775807), (-9223372036854775807);
SELECT SUM(v) FROM o;   -- Error: integer overflow
```

真实和等于 `i64::MAX`，装得下；但 SQLite 按顺序累加，第二步就溢出了。

增量维护的累加顺序**必然**与全量重算的扫描顺序不同，因此"增量成功、重算报错"或反之是可达状态——这与浮点加法不满足结合律是**同一类问题**，只是发生在整数上。

> **v0 的对策：把值域夹到不可能溢出，并把溢出明确列为不支持。**
>
> 具体约束：`|group 内所有值之和| < 2^62`。差分测试的生成器必须保证这一点（窄值域下自然满足）；`ivm_create_view` 不做静态检查（做不到），溢出时的行为**未定义**，文档如实声明。

这条与浮点的处理并列写在此处，是为了避免"只防了浮点"这个我已经犯过一次的错误。

#### 谓词的三值逻辑

`WHERE` 对 `NULL` 求值为 UNKNOWN，该行**不进入结果**；而 `WHERE NOT (...)` 同样不进入。已实测：`v` 取 `{1, NULL, 5}` 时，`WHERE v > 3` 命中 1 行，`WHERE NOT (v > 3)` 也只命中 1 行——两者加起来是 2 而不是 3。

因此谓词求值必须返回**三值**而非布尔，且"不通过"与"未知"在筛选语义上合并为同一种处理（都不进入结果）。v0 的实现把二者合并是正确的，但**不得据此认为 `NOT p` 等价于 `!p`**。

v0 允许的比较运算符白名单：`>`、`>=`、`<`、`<=`、`=`、`!=`、`IS NULL`、`IS NOT NULL`。不允许 `NOT`、`OR`、`LIKE`、`IN`、`BETWEEN` 与任何子查询——每多一个都要重新论证一次三值逻辑，而 v0 的目的不是覆盖 SQL。

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
__ivm_dep(view TEXT, tbl TEXT, PRIMARY KEY(view, tbl))   -- 视图依赖哪些基表
__ivm_delta_<table>(seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    w INTEGER, <表的所有列...>)            -- CDC，w 即 Z-set 权重
__ivm_state_<view>_<op>(key BLOB, val BLOB, w INTEGER,
                        PRIMARY KEY(key, val))            -- arrangement
__ivm_out_<view>(<输出列...>, __w INTEGER)                -- 物化输出，普通表
__ivm_progress(view TEXT, tbl TEXT, applied_seq INTEGER,
               PRIMARY KEY(view, tbl))                    -- 水位
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

### 7.2 delta 表的 GC

纯 SQL trigger 的设计让未加载扩展的连接也能被捕获（§8.1），代价是 **delta 表会无限增长**。没有 GC 的设计在 benchmark 里看不出问题，一上真实 workload 立刻爆——delta 表涨到千万、上亿行。

GC 水位由**依赖该基表的所有视图中最落后的那个**决定：

```sql
-- 对每张基表
gc_watermark(tbl) = (SELECT MIN(p.applied_seq)
                     FROM __ivm_progress p
                     JOIN __ivm_dep d ON d.view = p.view AND d.tbl = p.tbl
                     WHERE p.tbl = tbl);

DELETE FROM __ivm_delta_<tbl> WHERE seq <= gc_watermark(tbl);
```

`__ivm_dep` 存在的唯一理由就是这个查询：没有它就不知道"还有谁没消费完"。

**没有任何视图依赖某张被跟踪的表时**（最后一个视图被 DROP），该表的 trigger 与 delta 表一并删除，而不是让 delta 永久累积。

### 7.3 bootstrap 必须与 delta 水位原子

在已有数据的表上创建视图时，顺序错了会丢更新或重复应用：

```
1. 读 delta 表的高水位 H
2. 全量扫描基表，算出初始状态
3. 置 progress = H
```

**第 1、2 步必须在同一个读事务内**，否则两步之间发生的并发写会：progress 记为 H 但基表快照里没有它（丢更新），或者基表快照里有了却又会被 seq > H 的 delta 再应用一次（重复）。

SQLite 的读事务提供一致快照，因此把 `BEGIN` 包住这两步即可。这条必须写进实现而不只是写进文档——它属于"benchmark 看不见、真实 workload 立刻爆"的那一类。

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

**显式 refresh 不是妥协，它是本项目的永久 API，而且很可能是正确的抽象。**

三条理由：

1. **两条自动化路径都被 SQLite 的机制堵死**（上文），不存在"以后想办法自动化"的余地。
2. **它是测试矩阵成立的前提**——批次无关性（§9.1）只有在能精确控制维护时刻时才可测。
3. **批处理本身就是相对手写 trigger 的性能优势所在**，见下。

推荐的使用形态是把 refresh 放进应用自己的写事务：

```sql
BEGIN;
INSERT INTO orders ...;   -- ×10000
SELECT ivm_refresh('revenue');
COMMIT;
```

#### 为什么批处理是优势而不是缺陷

手写的行级 trigger 对一万行插入必然执行一万次聚合 UPDATE。而拿到整批 delta 的增量引擎可以先做 **consolidation**：

```
10000 条 raw Δ
      ↓  Z-set consolidate（相同行权重相加）
若只涉及 20 个 region
      ↓
20 次 group 状态更新
```

**这是显式 refresh 换来的、行级 trigger 结构上拿不到的东西**，也是本项目最可能成立的性能故事。因此 consolidation 被明确列为 M1 的内容，而非优化项。

### 8.3 控制面：已由 M-1 定稿——虚表 + 命令通道（FTS5 惯用法）

早期草案把控制面定成标量 UDF：

```sql
SELECT ivm_create_view('revenue', 'SELECT ...');   -- 内部要建表、建 trigger
SELECT ivm_refresh('revenue');                     -- 内部要写影子表
```

即：在一条正在 `sqlite3_step()` 的 `SELECT` 语句内部，用同一个连接做 DDL 和写入。这**不能靠"理论上应该可以"来赌**——SQLite 对 hook 的重入限制很严（commit/update hook 明确禁止在回调里再操作触发它的连接），application-defined function 的限制虽宽一些，但仍是在一条运行中的语句里递归使用同一连接。

**M-1 spike 在真实 cdylib loadable extension 上把标量 UDF（方案 A）与虚表命令通道（方案 B）各跑了一遍 10 个场景的矩阵**（rollback、嵌套事务、WAL、双连接、并发、销毁路径、未加载扩展的连接、以及方案 A 独有的"扫描中调用"），结果见 [2026-09-18-m-1-results.md](../../spikes/2026-09-18-m-1-results.md)。结论：**方案 B 在所有适用于它的场景上全绿，采用方案 B；控制面语法定稿。**

```sql
-- xCreate 建立影子表与 trigger，DDL 上下文天然正确
CREATE VIRTUAL TABLE revenue USING ivm(
    'SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region'
);

-- 命令通道：向与表同名的列写入
INSERT INTO revenue(revenue) VALUES ('refresh');

-- xFilter 读物化状态
SELECT * FROM revenue;

-- xDestroy：先删 trigger 再删影子表，顺序由扩展自己控制
DROP TABLE revenue;
```

参照：`CREATE VIRTUAL TABLE docs USING fts5(body)` 会经 xCreate 建出 `docs_data`、`docs_idx`、`docs_content`、`docs_docsize`、`docs_config` 五张影子表，`INSERT INTO docs(docs) VALUES('rebuild')` 是其命令入口。

方案 B 在三处优于标量 UDF，且三处都在 M-1 里得到了实测支持：

1. **DDL 发生在 SQLite 为之设计的上下文里**（`xCreate`）——场景 1–5 证实建表/建 trigger/写入在裸调用、显式事务、嵌套 savepoint、WAL 模式下全部成功，且回滚时（场景 3/4）影子表与 trigger 随事务一并消失，不留孤儿对象。
2. **视图成为 `sqlite_master` 认识的真实对象**——`CREATE VIRTUAL TABLE` 语句本身就在 `sqlite_master` 里，`SELECT * FROM revenue` 经 `xFilter` 正常可读。
3. **`DROP TABLE revenue` 经 `xDestroy` 自然清理影子表与 trigger，不需要额外的销毁 API**——场景 9 显示标量 UDF 方案没有这条性质：`DROP TABLE` 直接删掉影子表并不会级联删除指向它的 trigger，悬空的 trigger 会让**用户自己的基表**后续所有写入报错 `no such table`；而虚表方案的 `xDestroy` 由扩展自己控制顺序（先删 trigger 再删影子表），销毁后基表仍可正常写入。

M-1 还发现方案 A 有一个比场景 9 更严重、探针文档没有预判到具体形态的问题（场景 8）：若在扫描某张影子表的语句里调用 `ivm_refresh` 写同一张表（自引用），会导致游标不断看到自己刚插入的新行，陷入不报错、不减速的无界循环，实测到 46 万余行才被人工中断。方案 B 结构上不暴露这条路径——读走 `xFilter`（不写），写走独立的 `xUpdate` 语句（不在扫描回调里）。

并发场景（场景 6/7：另一连接同时在读/在写）两套方案行为完全对称，是 SQLite 标准锁语义（rollback-journal 下 `SQLITE_BUSY`，WAL 下读不挡写、写互斥写），没有为选型提供额外信号。场景 10 确认了 §8.1 的声明：纯 SQL trigger 对未加载扩展的连接同样生效。

**控制面到此定稿，本文档其余部分出现的 `ivm_create_view` / `ivm_refresh` 均指虚表命令通道形态的等价操作**（`CREATE VIRTUAL TABLE ... USING ivm(...)` / `INSERT INTO v(v) VALUES('refresh')`），不再是待定语法。

### 8.4 引擎接缝契约

M1 的引擎通过 `ivmlite-test` 的 `Engine` trait 接入差分测试框架。该 trait 的形状不是实现细节——它决定了哪些行为**能被测到**：

```rust
fn create_view(&mut self, &Schema, &ViewQuery, initial: &ZSet) -> Result<(), EngineError>;
fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError>;
fn refresh(&mut self) -> Result<(), EngineError>;
fn materialize(&mut self) -> Result<ZSet, EngineError>;
```

三条约束及其理由：

**`apply` 接收未合并的原始 Δ，不是 `ZSet`。** 同一行可以在同一批里出现多次，引擎必须自己决定要不要先 consolidate。若 harness 交出的是已合并的 `ZSet`，§8.2 所说的 consolidation——M1 的内容而非优化项，也是本项目最可能成立的性能故事——就从这个接缝上**结构性不可见**：引擎无论做没做合并，测试结果都一样。框架内有一个记录型引擎守着这条，喂进含重复行的批次并断言收到的是多条而非合并后的一条。

**`refresh` 与 `apply` 分离。** §8.2 规定显式 refresh 是永久 API 而非临时妥协，§9.1 的批次无关性也只有在维护时刻可控时才可测。参照实现 `NaiveRecompute` 因此**真的**分两阶段：`apply` 只堆 pending，`refresh` 才并进 base。若 `apply` 急切合并，`refresh` 便成空操作，任何忽略该契约的引擎都不会被抓到。

**`apply` 带表名。** §6.3 禁止 v0 做出会让 M2 的 join 返工的决定，而 join 需要多个基表。`materialize` 刻意**不**带 view 标识：多视图要到 M4 的级联视图才出现且形态未定，现在加属投机；多表是已排期的已知需求，加一个参数是当下最便宜的时刻。

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

批次无关性只有在维护时刻可被精确控制时才可测，自动维护模式下做不到。

> **但要如实说明它的实际地位：在「oracle 逐批比对」成立的前提下，跨模式比对作为失败检测路径是逻辑上不可达的。**
>
> 推演：`run` 在每个 refresh 点比对 oracle，所以 `run` 成功 ⇒ 该模式的最终状态等于 oracle 的期望；而 oracle 的期望只取决于最终基表状态，与 delta 如何分批无关；因此任意两个 `run` 成功的模式必然彼此相等。跨模式比对只可能在某个模式的 `run` 已经失败时才报错——它抓不到任何 `run` 抓不到的东西。
>
> 保留它的理由是成本近零，且一旦将来有人降低 `run` 的比对频率（那会是一次需要论证的改动），它会重新变得有意义。**不要把它计入独立的检测能力**，也不要为它编造一个不可能存在的失败用例。

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
  × group 基数 (10, 1k, 100k 个不同分组键)
→ 测量：应用一批 delta 并使所有视图变为最新所需时间
```

**group 基数是决定 IVM 赢不赢的首要参数，比基表规模本身更关键**，因此必须是显式维度而非硬编码常量：

| 配置 | 后果 |
|---|---|
| 10 个 group / 100 万行 | 视图只有 10 行，状态极小，每条 delta 都命中热 group，IVM 优势极大 |
| 100 万个 group / 100 万行 | 视图与基表等大，IVM 状态与数据等大，每条 delta 都是新建 group + retraction churn，优势基本消失 |

交叉点的位置随该参数剧烈移动。**只报告单一 group 基数下的数字，等同于自己挑了一个好看的点**，不构成结论。

四维全交叉是 144 个配置，过大。约定：**扫 group 基数时把视图数固定为 10**，不做全交叉；视图数的扫描单独在 group 基数 = 1k 时进行。

### 10.2 对照组按角色分层

| 角色 | 对照物 | 说明 |
|---|---|---|
| **下界** | 只写基表不维护 | 纯写入成本 |
| **怀疑者** | 手写 trigger 维护的汇总表 | 见下方的三级判据——**不是"必须打赢"** |
| **基线** | 朴素重跑 | 交叉点在此测量 |
| **同行** | Turso MV | 同宿主、同 DBSP，最公平 |
| **天花板** | `dbsp` crate 裸跑（手搓 circuit，不过 SQL，不落盘） | 与本项目的差距即为 SQLite / 存储税，诊断价值极高 |
| **参考** | duckDBSP / OpenIVM / pg_ivm | 跨宿主，只作背景不作裁决 |

"怀疑者"这一栏对应的是对 v0 最直接的质疑：单表 GROUP BY + SUM/COUNT 就是人们手写了三十年的 trigger 汇总表。必须正面回答——但**判据不是"必须打赢"**。

#### 对手写 trigger 的三级判据

早期草案写的是"v0 必须打赢手写 trigger，否则没有故事"。**这个判据是错的。** 针对 `GROUP BY region → SUM(amount)` 手写的专用 trigger，本身就是这条查询手工编译后的最优实现之一；而通用引擎必须为通用性付费：泛化的 delta 表示、序列化、arrangement 查找、算子分派、progress 跟踪、CDC 日志。**打不赢它不等于没有价值。**

正确的判据分三级：

| 级别 | 判据 | 含义 |
|---|---|---|
| **必须** | `ivmlite ≪ 全量重算` | 达不到则项目前提不成立 |
| **期望** | `ivmlite` 接近手写 trigger | 通用性的代价在可接受范围内 |
| **额外惊喜** | 大批量 Δ 下 `ivmlite` **优于**手写行级 trigger | consolidation 带来的结构性优势 |

第三级是有机会达成的，而且机会正来自 §8.2 论证的批处理语义：一万次插入若只涉及 20 个 region，手写行级 trigger 要执行一万次聚合 UPDATE，而拿到整批 delta 的引擎 consolidate 后只需 20 次。

**因此 benchmark 必须包含"大批量 Δ + 低 group 基数"这个格子**——它是第三级判据唯一可能出现的地方，也是把手写 trigger 正确定位为"专用上界"而非"必须翻越的门槛"之后，真正值得测的东西。

### 10.3 方法论约束

1. **跨宿主的绝对耗时不可比**（DuckDB vs SQLite 量的是宿主而非 IVM）。跨宿主只比**同宿主内的加速比** `朴素重跑 / 增量`。
2. **CI 中只保留 same-host 对照组**；跨宿主对比做成一次性 writeup，不进 CI（否则必然腐烂）。
3. **所有对照组必须维护完全相同的视图集合。** 若"手写 trigger"只能表达单一形状而"朴素重跑"跑的是另一批查询，测出的倍数无法用于 §10.4 的结论。视图集合的上限由**表达能力最弱的那个对照组**决定——v0 即 `GROUP BY <单列> → SUM, COUNT`，所有对照组一律用这一形状的 N 份副本。
4. **所有对照组必须在计时开始前完成初始状态构建。** 在基表已有数据之后才创建空的汇总表，得到的是一个从不完整的视图，其维护成本也不具代表性。每条基线都要先完成一次全量 bootstrap，再开始测量增量成本。
5. **测试数据必须有稳定主键，删改按主键定位。** 按全部列的值去找行会退化成全表扫描（`EXPLAIN QUERY PLAN` 显示 `SCAN`），使耗时随基表规模线性增长——而基表规模项正是这条 benchmark 唯一要证明的东西，被扫描淹没后结论归零。
6. **绝不把其他项目公开发布的数字放进对比表。** 不同硬件、不同数据、不同查询、不同测量方法，别人博客或论文里的毫秒数与本项目的数字之间没有可比性。要与 Turso 等系统对比，就必须在同一台机器上、用同一份 workload 亲自跑一遍。他人发布的数字只有一个合法用途：判断自己的量级是否离谱到说明某处搞错了——不能进结论。
7. **workload 必须是可移植产物，不得写死在 runner 里。** schema DDL、视图 SQL、数据生成参数、更新 trace 定义在一个独立文件中，生成器可将其导出为 CSV/SQL 供任何引擎加载。runner 是每引擎一份，workload 只有一份。否则每接入一个对比系统都要重新设计一次 benchmark，而重新设计过的 benchmark 之间不可比。这条同样约束**格子推导规则本身**——若被测矩阵（扫多少种基表规模、批大小、视图数、group 基数，以及怎样从这些维度构造出一个具体配置）活在 runner 代码里，外部 runner 就得重新实现这份推导逻辑，逐字段猜对，这与写死 workload 是同一种失败，只是换了一层：因此格子推导规则属于 workload 定义本身，而不是某一份 runner。

### 10.4 要得出的结论

IVM 耗时应随 **Δ 大小**增长、几乎不随**基表规模**增长；朴素重跑随基表规模线性增长。

**输出不是一个数字，而是一张面（surface）：**

```
基表规模 × Δ 大小 × group 基数 × 视图数 × refresh 频率
                      ↓
              全量重算耗时 / 增量耗时
```

早期草案预先规定"交叉点落在 100 万行以上则对 SQLite 没意义"。**该阈值已删除**——它是拍脑袋定的，而且把一个五维问题压成了一个数。同一套实现在"10 个 group、大批量 Δ"和"90 万个 group、单行 Δ"下是两个完全不同的结论，不存在单一交叉点。

> **benchmark 的设计仍必须能够证伪本项目，只是判据换成了形状而非数字：**
>
> **若这张面上不存在任何一个区域，使增量相对全量重算有实质优势（比值 > 2），则项目前提不成立。** 反过来，若优势区域存在，就照实报告它落在哪里——包括"只在极窄的一角成立"这种结论。
>
> 一个只会得出好结论的 benchmark 没有价值；一个预先规定了好结论长什么样的 benchmark 同样没有价值。

### 10.5 次要指标

- **写放大**——安装本扩展后，trigger 使**所有**写入变慢，哪怕从不读视图。这是 IVM 的隐藏税，必须量化，否则收益数字是假的。
- 空间放大：state + delta 表 vs 基表
- bootstrap 耗时

### 10.6 已知简化（M0/M1 接受，M2 消除）

这三条都会让 M0/M1 的数字偏离真实负载，记录在此以免日后把它们当成结论：

- **数据分布是均匀的，不是 Zipf。** 真实数据里少数热 group 吃掉大部分更新。这会显著改变缓存行为，也直接影响 M4 内存 arrangement 的收益评估——均匀分布下内存缓存的价值被低估。
- **更新是均匀散开的，没有局部性。** 真实负载的更新集中打热 group。
- **无法运行任何标准基准的查询。** TPC-H 的查询需要 join，Nexmark 的查询大多需要 join 与窗口，而 v0 只有单表 GROUP BY。因此 M0/M1 只能用合成数据，跨系统对比在 M2 之前无法成立。

### 10.7 标准基准：M2 起接入 Nexmark

join 落地后接 **Nexmark**——流式/增量系统的事实标准。Feldera 仓库内置 Nexmark benchmark，RisingWave 公开发布 Nexmark 结果，因此用它做对比时别人的数字才有参照系（仍须自行运行，见 §10.3 第 6 条）。

现在即确定这一目标，目的是让 plan IR 与算子接口不往与之不兼容的方向漂移。

### 10.8 建立时机

**benchmark harness 先用朴素重跑实现填充"待测引擎"的位置**，于是第一天就有完整基线曲线，实现 core 的每一步都有实时对比，而不是做完才知道快慢。

---

## 11. Roadmap

> **顺序约束：测试框架与 benchmark 骨架必须在 core 之前建立；而 SQLite 扩展机制的探针（M-1）必须在 M1 之前。**

### M-1 — SQLite 扩展机制 spike（最先做）—— ✅ 已完成

**这是一个 spike，产出是结论不是代码。** 目的是在写任何引擎之前搞清楚控制面到底能不能按设想工作——如果不能，现在换比 core 写完再换便宜几个数量级。

在**真实的 cdylib 扩展**（不是宿主语言的 sqlite3 绑定）上，把两套控制面各跑了一遍：

- **方案 A**：标量 UDF 内做 DDL 与写入
- **方案 B**：`CREATE VIRTUAL TABLE ... USING ivm(...)`，xCreate 建影子表与 trigger，`INSERT INTO v(v) VALUES('refresh')` 作命令通道（FTS5 惯用法，主候选）

每套都覆盖了：`CREATE TABLE` / `CREATE TRIGGER` / 写影子表 / rollback / 嵌套事务 / WAL 模式 / 两个连接并发 / `DROP` 清理，方案 A 还额外覆盖了"扫描中调用"。

> **完成判定：控制面定稿，§8.3 从"待验证"改为结论。** 结果：方案 B 全绿，采用方案 B；方案 A 暴露了两个问题（销毁路径留孤儿 trigger 毒死基表、扫描中自引用写入导致无界循环），记录为不采用 B 时的已知坑，不是"两套都有问题"意义上的阻塞项。完整矩阵与证据见 [2026-09-18-m-1-results.md](../../spikes/2026-09-18-m-1-results.md)。

M-1 与 M0 相互独立（M0 是纯 Rust 的测试与 benchmark 骨架，不碰扩展 API），M-1 先做的原因（结论可能改写 §7 与 §8）已经落地：§8.3 已更新为结论；§7.3 的 bootstrap 原子性论证经场景 3/4 复核后维持不变，无需重写。

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
- **delta consolidation**（§8.2）：raw Δ 先按 Z-set 合并同一行的权重，再进算子。这是本项目最可能成立的性能故事，属于 M1 的内容而非后续优化
- `ivmlite-sql`：`sqlparser-rs` → IR，Catalog trait，子集外硬报错
- `ivmlite-sqlite`：cdylib、M-1 定稿的控制面、trigger DDL、shadow table
- **bootstrap 的水位原子性**（§7.3）：高水位与基表快照必须在同一读事务内
- **delta 表 GC**（§7.2）：`__ivm_dep` + 最落后视图水位 + 视图全部 DROP 后清理 trigger 与 delta 表

**v0 限制**：根算子必须是带非空 GROUP BY 的 Aggregate（§5.2）；无全局聚合；STRICT table only 且拒绝 `ANY` 列；BINARY collation only；group-by key 只能是裸列；无浮点聚合；整数溢出未定义；比较运算符限于白名单；显式 refresh；单表（无 join）；INSERT / DELETE / UPDATE 全部支持。

> **完成判定：M0 的全部测试绿；§10.4 那张面跑出来；写放大有明确数字。数字难看也算完成。**

### M2 — Join（第一个可辩护的里程碑）

- Join 算子 + 两侧 arrangement
- query 生成器扩展到两表
- Turso 接入，一次 setup 兼顾两件事：**正确性交叉验证**（§9.1 第四层）与**性能同行对比**（§10.2）
- 接入 **Nexmark**（§10.7）——join 落地后才具备运行条件
- 数据分布扩展为 Zipf，更新扩展出局部性（消除 §10.6 的前两条简化）
- 观察并记录 join 状态爆炸（两侧都需保存全量）

### M3 — 多连接语义与维护策略

**原先此处写的"vtab 自动 drain"已删除——它与 §8.2 的论证直接矛盾。** §8.2 证明了读事务不能写，所以 `SELECT * FROM view` 无法顺手把 pending delta 应用进去；而 trigger 内立即维护又因为 SQLite 只有行级 trigger 会把一次万行插入变成一万次维护。两条路都堵死，M3 不该承诺一个 §8 已经排除的东西。

本里程碑改为：

- 多连接下的 staleness 语义与 `applied_seq` 水位协调
- delta 表 GC 的并发安全（见 §7.2）
- 维护触发策略的**人体工学改进**，而非自动化幻觉：例如提供 `ivm_refresh_all()`、把 refresh 挂进应用自己的 commit 流程的推荐写法

**显式 refresh 是永久 API，不是 v0 的临时妥协**（见 §8.2）。

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

1. **根算子必须是带非空 GROUP BY 的 Aggregate**；不支持无聚合的视图，也不支持全局聚合（§5.2）
2. 仅 STRICT table，且**拒绝 `ANY` 列**（STRICT 本身不排除 `ANY`，见 §7.1）；列类型白名单为 `INTEGER` / `TEXT`
3. 仅 BINARY collation；检测手段是从 `sqlite_master` 取建表语句匹配 `COLLATE`，属保守的过度拒绝（见 §7.1）
4. group-by key 仅支持裸列，不支持表达式
5. 无浮点聚合
6. **整数溢出行为未定义**；要求 group 内和的绝对值 < 2^62（§6.1）
7. 比较运算符限于 `>` `>=` `<` `<=` `=` `!=` `IS NULL` `IS NOT NULL`；无 `NOT` / `OR` / `LIKE` / `IN` / `BETWEEN` / 子查询（§6.1）
8. 无 join
9. 无 MIN / MAX / DISTINCT
10. 需显式 refresh——这是永久 API 而非临时妥协（§8.2）
11. delta 表捕获全部列，宽表上有空间浪费
12. 所有写入承担 trigger 写放大，即使从不读视图
13. 无法在浏览器或 iOS 系统 SQLite 上加载
