mod baseline;
mod plot;

use std::path::Path;

use baseline::{apply, install_trigger_view, recompute_all, seed_base, Baseline};
use ivmlite_workload::Workload;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct Record {
    pub baseline: &'static str,
    pub views: usize,
    pub base_rows: usize,
    pub batch: usize,
    pub cardinality: usize,
    pub apply_ms: f64,
    pub maintain_ms: f64,
}

/// 跑一个已经完全具体化的矩阵格子。`cell` 由 `Workload::cells()` 产出，
/// 此函数不再改动它——`base_rows` / `group_cardinality` / `batch_size` /
/// `views` 四个维度全部由 `ivmlite-workload` 决定（spec §10.3 第 7 条），
/// `main.rs` 只负责按基线跑它、计时、记录结果。
fn run_one(b: Baseline, cell: &Workload) -> rusqlite::Result<Record> {
    let conn = Connection::open_in_memory()?;

    // ---- 以下全部不计时：建立初始状态 ----
    seed_base(&conn, cell)?;
    if b == Baseline::HandWrittenTrigger {
        for v in &cell.views {
            install_trigger_view(&conn, &cell.schema.table, v)?;
        }
    }
    let ops = cell.update_trace();

    // ---- 计时区间 ----
    let apply_ms = apply(&conn, &cell.schema.table, &ops)?;
    let maintain_ms = match b {
        // trigger 的成本已计入 apply_ms——那正是写放大
        Baseline::NoMaintenance | Baseline::HandWrittenTrigger => 0.0,
        Baseline::NaiveRecompute => recompute_all(&conn, cell)?,
    };

    Ok(Record {
        baseline: b.label(),
        views: cell.views.len(),
        base_rows: cell.data.base_rows,
        batch: cell.updates.batch_size,
        cardinality: cell.data.group_cardinality,
        apply_ms,
        maintain_ms,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Workload::load(Path::new("workloads/m0-baseline.toml"))?;
    let cells = base.cells();
    let mut records: Vec<Record> = Vec::new();

    let baselines = [
        Baseline::NoMaintenance,
        Baseline::HandWrittenTrigger,
        Baseline::NaiveRecompute,
    ];

    // 矩阵的结构（两次扫描、`card > base_rows` 的跳过规则、视图阈值公式）
    // 现在完全活在 `Workload::cells()` 里（spec §10.3 第 7 条），这里只是
    // 按基线遍历它产出的格子。跳过的格子不再在这里单独记一行 stderr——
    // `cells()` 直接不产出它们，`main.rs` 没有 `card > rows` 这条判断可用
    // 来识别"本该有但被跳过"的格子，硬凑一份就是把已经搬走的规则在这里
    // 重新实现一遍；`docs/bench/README.md` 已经记录了这个跳过（`card=100000
    // > base_rows=10000`），不需要 runner 再重复一次。
    for b in baselines {
        for cell in &cells {
            records.push(run_one(b, cell)?);
        }
    }

    println!("baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms");
    for r in &records {
        println!(
            "{},{},{},{},{},{:.3},{:.3}",
            r.baseline, r.views, r.base_rows, r.batch, r.cardinality, r.apply_ms, r.maintain_ms
        );
    }

    // 每个 group 基数各出一张图——交叉点随该参数剧烈移动，只出一张等于
    // 自己挑了个好看的点（spec §10.1）。基数取值与固定视图数直接读
    // `[matrix]`，不再是 main.rs 里的常量。
    let matrix = base
        .matrix
        .as_ref()
        .expect("workloads/m0-baseline.toml 缺少 [matrix] 段");
    for card in matrix.group_cardinalities.clone() {
        let path = format!("docs/bench/m0-baseline-card{card}.svg");
        match plot::write_svg(Path::new(&path), &records, matrix.fixed_views, 100, card) {
            Ok(()) => eprintln!("图已写入 {path}"),
            // 某个 group 基数在所有基表规模下都被跳过时没有数据点，
            // 这不是错误——照实说明并继续。
            Err(e) => eprintln!("跳过 card={card} 的出图: {e}"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从 `docs/bench/m0-baseline.csv` 里读出的一行，只取跟矩阵格子相关的
    /// 四个维度（忽略 baseline 名字与两个耗时列——它们不是 `cells()` 的
    /// 产出）。
    type CsvCellKey = (usize, usize, usize, usize); // (views, base_rows, batch_size, group_cardinality)

    /// 手写一个最小 CSV 解析：这份文件里没有引号转义或内嵌逗号，字段全是
    /// 简单的标识符/数字，不值得为它引入一个 csv 依赖。
    fn read_csv_cell_keys(path: &Path) -> std::collections::BTreeSet<CsvCellKey> {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
        let mut lines = text.lines();
        let header = lines.next().expect("CSV 至少要有表头");
        assert_eq!(
            header, "baseline,views,base_rows,batch_size,group_cardinality,apply_ms,maintain_ms",
            "CSV 表头形状变了，下面按位置取字段的假设不再成立"
        );

        lines
            .filter(|l| !l.is_empty())
            .map(|line| {
                let fields: Vec<&str> = line.split(',').collect();
                assert_eq!(fields.len(), 7, "CSV 行字段数不对: {line}");
                let views: usize = fields[1].parse().unwrap_or_else(|_| panic!("{line}"));
                let base_rows: usize = fields[2].parse().unwrap_or_else(|_| panic!("{line}"));
                let batch_size: usize = fields[3].parse().unwrap_or_else(|_| panic!("{line}"));
                let group_cardinality: usize =
                    fields[4].parse().unwrap_or_else(|_| panic!("{line}"));
                (views, base_rows, batch_size, group_cardinality)
            })
            .collect()
    }

    /// M0 review 的 finding I7：另一个引擎的 runner 加载
    /// `workloads/m0-baseline.toml` 后，必须能重新推导出与已发布的
    /// `docs/bench/m0-baseline.csv` 完全一致的格子集合——这正是
    /// `ivmlite-workload` 存在的意义（spec §10.3 第 7 条）。这条测试是那句话
    /// 唯一的证明：把已发布 CSV 里出现过的 `(views, base_rows, batch_size,
    /// group_cardinality)` 四元组去重，与 `base.cells()` 产出的同一组四元组
    /// 做集合相等比较——数量相同、成员相同，没有多的也没有少的。
    ///
    /// 不重新跑 benchmark（那要约 9 分钟）：这里只读已经提交的 CSV 文件，
    /// 不改动它一个字节；它是这条测试要对照的既有事实（fixture）。
    #[test]
    fn cells_reproduce_exactly_the_published_csv_matrix() {
        let workload_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m0-baseline.toml");
        let csv_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/bench/m0-baseline.csv");

        let base = Workload::load(&workload_path).expect("发布的 workload 文件必须可解析");
        let derived: std::collections::BTreeSet<CsvCellKey> = base
            .cells()
            .iter()
            .map(|c| {
                (
                    c.views.len(),
                    c.data.base_rows,
                    c.updates.batch_size,
                    c.data.group_cardinality,
                )
            })
            .collect();

        let published = read_csv_cell_keys(&csv_path);

        let missing: Vec<_> = published.difference(&derived).collect();
        let extra: Vec<_> = derived.difference(&published).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "cells() 与已发布 CSV 的格子集合不一致:\n缺失（CSV 有、cells() 没有）: {missing:?}\n多余（cells() 有、CSV 没有）: {extra:?}"
        );
        assert_eq!(
            derived.len(),
            published.len(),
            "cells() 产出 {} 个不同格子, CSV 有 {} 个不同格子",
            derived.len(),
            published.len()
        );
    }
}
