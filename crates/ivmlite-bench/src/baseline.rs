use std::time::Instant;

use ivmlite_workload::{TraceOp, ViewSpec, Workload};
use rusqlite::{Connection, Statement};

/// spec §10.2 的三条 same-host 对照组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// 下界：只写基表，完全不维护视图。纯写入成本。
    NoMaintenance,
    /// 怀疑者：手写 trigger 维护汇总表。spec §10.2 的判据是三级的，**不是**
    /// "必须打赢"：**必须** `ivmlite ≪ 全量重算`；**期望** `ivmlite` 接近
    /// 手写 trigger；**额外惊喜** 大批量 Δ 下 `ivmlite` 优于手写行级
    /// trigger（consolidation 带来的结构性优势）。手写 trigger 本身就是
    /// 这条查询手工编译后的最优实现之一，通用引擎要为通用性（泛化的 delta
    /// 表示、序列化、arrangement 查找、算子分派、progress 跟踪）付费，
    /// 打不赢它不等于没有价值。
    HandWrittenTrigger,
    /// 基线：每批 delta 之后把所有视图 SQL 重跑一遍。交叉点在这里测量。
    NaiveRecompute,
}

impl Baseline {
    pub fn label(self) -> &'static str {
        match self {
            Baseline::NoMaintenance => "no_maintenance",
            Baseline::HandWrittenTrigger => "hand_written_trigger",
            Baseline::NaiveRecompute => "naive_recompute",
        }
    }
}

/// 建基表并灌入初始数据。不计时。
///
/// 表结构来自 workload，其中 `id INTEGER PRIMARY KEY` 是硬性要求：
/// spec §10.3 第 5 条——按全部列的值定位行没有可用索引，`EXPLAIN QUERY PLAN`
/// 会显示 `SCAN orders`，使删改耗时随基表规模线性增长，而"增量成本不随基表
/// 规模增长"正是这条 benchmark 唯一要证明的东西。
pub fn seed_base(conn: &Connection, w: &Workload) -> rusqlite::Result<()> {
    conn.execute_batch(&w.schema.ddl)?;
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{}\"(id, region, amount) VALUES (?1, ?2, ?3)",
            w.schema.table
        ))?;
        for (id, region, amount) in w.rows() {
            ins.execute((id, &region, amount))?;
        }
    }
    tx.commit()
}

/// 建汇总表 →**先全量 bootstrap**→ 再建 trigger。顺序不可颠倒。
///
/// spec §10.3 第 4 条：在基表已有数据之后才创建空汇总表，得到的是一个永远
/// 不完整的视图，其维护成本也不具代表性。而 trigger 必须在 bootstrap **之后**
/// 创建，否则 bootstrap 那条 INSERT ... SELECT 会被 trigger 重复计入。
///
/// 注意汇总表的 `k` 列声明为 `TEXT` 而非 `ANY`：STRICT 表允许 `ANY` 列逐行
/// 混存类型，`1` 与 `'1'` 会分裂成两个 group（spec §7.1）。
pub fn install_trigger_view(conn: &Connection, table: &str, v: &ViewSpec) -> rusqlite::Result<()> {
    let t = v.table();
    let k = v.threshold;
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE "{t}" (
            k TEXT    NOT NULL PRIMARY KEY,
            s INTEGER NOT NULL,
            c INTEGER NOT NULL
        ) STRICT;

        INSERT INTO "{t}"(k, s, c)
            SELECT region, SUM(amount), COUNT(*)
            FROM "{table}" WHERE amount > {k} GROUP BY region;

        CREATE TRIGGER "{t}_ins" AFTER INSERT ON "{table}"
        WHEN NEW.amount > {k} BEGIN
            INSERT INTO "{t}"(k, s, c) VALUES (NEW.region, NEW.amount, 1)
            ON CONFLICT(k) DO UPDATE SET s = s + NEW.amount, c = c + 1;
        END;

        CREATE TRIGGER "{t}_del" AFTER DELETE ON "{table}"
        WHEN OLD.amount > {k} BEGIN
            UPDATE "{t}" SET s = s - OLD.amount, c = c - 1 WHERE k = OLD.region;
            DELETE FROM "{t}" WHERE k = OLD.region AND c = 0;
        END;
        "#
    ))
}

/// The statements `apply` executes, compiled before the timer starts.
///
/// They must be prepared **after every trigger exists**. Creating a trigger
/// changes the schema, which invalidates statements compiled before it, and
/// SQLite compiles a trigger's body into the statement that fires it. Timing a
/// `prepare` therefore measures compilation, not maintenance — and each matrix
/// cell runs exactly once, so that first-call cost is all a cell ever records.
/// Measured with 10 trigger views and a one-row batch: the first call cost
/// 0.129 ms against a 0.009 ms steady state (external review P2-3).
pub struct ApplyStatements<'c> {
    insert: Statement<'c>,
    delete: Statement<'c>,
}

impl<'c> ApplyStatements<'c> {
    pub fn prepare(conn: &'c Connection, table: &str) -> rusqlite::Result<Self> {
        Ok(Self {
            insert: conn.prepare(&format!(
                "INSERT INTO \"{table}\"(id, region, amount) VALUES (?1, ?2, ?3)"
            ))?,
            delete: conn.prepare(&format!("DELETE FROM \"{table}\" WHERE id = ?1"))?,
        })
    }
}

/// Apply one batch of changes and return the elapsed milliseconds. The timed
/// region covers execution and the commit — committing is a real cost — but
/// not statement compilation, which `ApplyStatements::prepare` does up front.
///
/// For the HandWrittenTrigger baseline the trigger work lands here, so
/// `apply_ms(trigger) - apply_ms(no_maintenance)` is the write amplification
/// spec §10.5 asks for.
pub fn apply(
    conn: &Connection,
    stmts: &mut ApplyStatements<'_>,
    ops: &[TraceOp],
) -> rusqlite::Result<f64> {
    let start = Instant::now();
    let tx = conn.unchecked_transaction()?;
    for op in ops {
        match op {
            TraceOp::Insert { id, region, amount } => {
                stmts.insert.execute((id, region, amount))?;
            }
            TraceOp::Delete { id } => {
                stmts.delete.execute((id,))?;
            }
        }
    }
    tx.commit()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// One compiled statement per view, prepared before the timer starts — the
/// same reasoning as `ApplyStatements`, applied to the naive baseline so both
/// sides of the comparison exclude compilation. `prepare_cached` would not
/// have been enough: rusqlite's statement cache holds 16 entries by default
/// and a cell can have 200 views.
pub struct RecomputeStatements<'c>(Vec<Statement<'c>>);

impl<'c> RecomputeStatements<'c> {
    pub fn prepare(conn: &'c Connection, w: &Workload) -> rusqlite::Result<Self> {
        w.views
            .iter()
            .map(|v| conn.prepare(&v.sql(&w.schema.table)))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map(Self)
    }
}

/// Naive recompute: run every view's SQL once and drain its result set.
///
/// The results are deliberately **not** written back to a table. That biases
/// the comparison toward naive recompute on purpose: if incremental maintenance
/// cannot beat a recompute that only reads, the conclusion is beyond dispute.
pub fn recompute_all(stmts: &mut RecomputeStatements<'_>) -> rusqlite::Result<f64> {
    let start = Instant::now();
    for stmt in &mut stmts.0 {
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDL: &str = "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT";

    fn trigger_rows(conn: &Connection, v: &ViewSpec) -> Vec<(String, i64, i64)> {
        let mut stmt = conn
            .prepare(&format!("SELECT k, s, c FROM \"{}\" ORDER BY k", v.table()))
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// SQL 与 trigger 维护的 (k,s,c) 用相同的顺序编码，直接从视图 SQL 里读出
    /// `region, SUM(amount), COUNT(*)` 再排序，与 trigger 表逐行比对。
    fn direct_query_rows(conn: &Connection, v: &ViewSpec, table: &str) -> Vec<(String, i64, i64)> {
        let sql = format!("{} ORDER BY region", v.sql(table));
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// I6：此前 `ivmlite-bench` 没有任何 `#[test]`，没有东西验证手写 trigger
    /// 维护出来的汇总表在重放完 trace 之后与直接对视图 SQL 求值逐行相等。
    /// `docs/bench/README.md` 里每一个头条比值都以这条基线的耗时为分母——
    /// trigger 若算错（比如 `WHEN OLD.amount > k` 的守卫、或 `c = 0` 的清理
    /// 逻辑有误），这些数字量的就是错的工作量。
    ///
    /// 场景刻意让基表在 `install_trigger_view` 之前就已经有数据（模拟
    /// `main.rs::run_one` 里 `seed_base` 先于 `install_trigger_view` 的真实
    /// 调用顺序），这样才能同时验证 bootstrap 语句本身在起作用——
    /// spec §10.3 第 4 条：trigger 必须在 bootstrap 之后创建，而 bootstrap
    /// 必须真的把已有数据算进去，否则汇总表会永远缺失历史数据（见下面的
    /// `install_trigger_view_without_bootstrap_would_be_incomplete` 对比）。
    #[test]
    fn trigger_maintained_table_matches_direct_query_after_seed_and_updates() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DDL).unwrap();

        // 先灌入"已有数据"，再装 trigger——这是 install_trigger_view 的
        // bootstrap 语句必须正确处理的场景。
        let seed_rows = [
            (0i64, "a", 10i64),
            (1, "a", 20),
            (2, "b", 5),
            (3, "b", 50),
            (4, "c", 1),
        ];
        {
            let tx = conn.unchecked_transaction().unwrap();
            {
                let mut ins = tx
                    .prepare("INSERT INTO orders(id, region, amount) VALUES (?1, ?2, ?3)")
                    .unwrap();
                for (id, region, amount) in seed_rows {
                    ins.execute((id, region, amount)).unwrap();
                }
            }
            tx.commit().unwrap();
        }

        let view = ViewSpec {
            id: 0,
            threshold: 8,
        };
        install_trigger_view(&conn, "orders", &view).unwrap();

        // 重放一批既有 insert 又有 delete 的更新，一律用 baseline::apply——
        // 与 benchmark 实际使用的路径完全一致。
        let ops = vec![
            TraceOp::Insert {
                id: 5,
                region: "a".into(),
                amount: 30,
            },
            TraceOp::Delete { id: 2 }, // amount=5，本就不过 threshold
            TraceOp::Insert {
                id: 6,
                region: "c".into(),
                amount: 100,
            },
        ];
        let mut stmts = ApplyStatements::prepare(&conn, "orders").unwrap();
        apply(&conn, &mut stmts, &ops).unwrap();
        drop(stmts);

        let got = trigger_rows(&conn, &view);
        let want = direct_query_rows(&conn, &view, "orders");
        assert_eq!(
            got, want,
            "trigger 维护的汇总表必须与直接对视图 SQL 求值逐行相等"
        );
        // 顺带钉死具体数字，避免两边用同一个（可能都错的）SQL 互相"印证"。
        assert_eq!(
            got,
            vec![
                ("a".to_string(), 60, 3),
                ("b".to_string(), 50, 1),
                ("c".to_string(), 100, 1),
            ]
        );
    }

    /// I6 的第二部分：bootstrap 必须发生在 trigger 创建**之前**
    /// （spec §10.3 第 4 条）。用手工拼接的 SQL 模拟"漏掉 bootstrap 语句"
    /// 这个 mutation，证明如果真出现这个 bug，本测试组里的第一个测试会
    /// 检测出差异——这里直接断言"没有 bootstrap 的 trigger 表"与"有
    /// bootstrap 的 trigger 表"不同，把这条方法论约束钉成一个会变红的测试，
    /// 而不是只靠注释自证。
    #[test]
    fn install_trigger_view_without_bootstrap_would_be_incomplete() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DDL).unwrap();
        for (id, region, amount) in [(0i64, "a", 10i64), (1, "a", 20), (2, "b", 50)] {
            conn.execute(
                "INSERT INTO orders(id, region, amount) VALUES (?1, ?2, ?3)",
                (id, region, amount),
            )
            .unwrap();
        }

        let view = ViewSpec {
            id: 0,
            threshold: 0,
        };
        // 手工拼出"漏掉 bootstrap INSERT...SELECT"的版本：只建表 + 建 trigger。
        let t = view.table();
        conn.execute_batch(&format!(
            r#"
            CREATE TABLE "{t}" (k TEXT NOT NULL PRIMARY KEY, s INTEGER NOT NULL, c INTEGER NOT NULL) STRICT;
            CREATE TRIGGER "{t}_ins" AFTER INSERT ON "orders" WHEN NEW.amount > 0 BEGIN
                INSERT INTO "{t}"(k, s, c) VALUES (NEW.region, NEW.amount, 1)
                ON CONFLICT(k) DO UPDATE SET s = s + NEW.amount, c = c + 1;
            END;
            "#
        ))
        .unwrap();

        let without_bootstrap = trigger_rows(&conn, &view);
        let want = direct_query_rows(&conn, &view, "orders");
        assert_ne!(
            without_bootstrap, want,
            "跳过 bootstrap 时汇总表必须缺失历史数据——如果这里相等，说明这个\
             对比场景本身没有测到 bootstrap 缺失的效果"
        );
        assert!(
            without_bootstrap.is_empty(),
            "没有 bootstrap 语句时，装 trigger 之前已存在的三行数据不会被追溯计入"
        );
    }
}
