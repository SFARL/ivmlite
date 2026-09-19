use std::time::Instant;

use ivmlite_workload::{TraceOp, ViewSpec, Workload};
use rusqlite::Connection;

/// spec §10.2 的三条 same-host 对照组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// 下界：只写基表，完全不维护视图。纯写入成本。
    NoMaintenance,
    /// 怀疑者：手写 trigger 维护汇总表。v0 引擎必须打赢它，否则没有故事。
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

/// 应用一批变更并返回毫秒数。计时包含 commit——提交成本是真实成本。
///
/// 对 HandWrittenTrigger 基线，trigger 的开销天然计入这里，因此
/// `apply_ms(trigger) − apply_ms(no_maintenance)` 就是 spec §10.5 要求的写放大。
pub fn apply(conn: &Connection, table: &str, ops: &[TraceOp]) -> rusqlite::Result<f64> {
    let start = Instant::now();
    let tx = conn.unchecked_transaction()?;
    {
        let mut ins = tx.prepare_cached(&format!(
            "INSERT INTO \"{table}\"(id, region, amount) VALUES (?1, ?2, ?3)"
        ))?;
        let mut del = tx.prepare_cached(&format!("DELETE FROM \"{table}\" WHERE id = ?1"))?;
        for op in ops {
            match op {
                TraceOp::Insert { id, region, amount } => {
                    ins.execute((id, region, amount))?;
                }
                TraceOp::Delete { id } => {
                    del.execute((id,))?;
                }
            }
        }
    }
    tx.commit()?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// 朴素重跑：把每个视图的 SQL 各跑一遍并耗尽结果集。
///
/// 这里**不把结果写回表**，是刻意偏向朴素重跑的保守选择——若增量方案连
/// "只读不写"的朴素重跑都赢不了，结论就无可辩驳。
pub fn recompute_all(conn: &Connection, w: &Workload) -> rusqlite::Result<f64> {
    let start = Instant::now();
    for v in &w.views {
        let mut stmt = conn.prepare_cached(&v.sql(&w.schema.table))?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
