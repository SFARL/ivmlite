mod baseline;
mod plot;

use std::path::Path;

use baseline::{apply, install_trigger_view, recompute_all, seed_base, Baseline};
use ivmlite_workload::{ViewSpec, Workload};
use rusqlite::Connection;

const BASE_ROWS: [usize; 3] = [10_000, 100_000, 1_000_000];
const BATCH_SIZES: [usize; 4] = [1, 10, 100, 1000];
const VIEW_COUNTS: [usize; 4] = [1, 10, 50, 200];
const GROUP_CARDINALITIES: [usize; 3] = [10, 1_000, 100_000];

/// 扫 group 基数时固定的视图数，扫视图数时固定的 group 基数。
///
/// 四维全交叉是 144 个配置，过大。spec §10.1 约定这两个固定值，于是两次扫描
/// 各 36 个配置，且都穿过同一个共同点 (views=10, cardinality=1k)，两组图可以
/// 对齐着读。
const FIXED_VIEWS: usize = 10;
const FIXED_CARDINALITY: usize = 1_000;

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

/// 从基准 workload 派生出一个具体配置。
///
/// 视图形状受限于表达能力最弱的对照组——手写 trigger（spec §10.3 第 3 条），
/// 因此这里只改视图**数量**与阈值，不改形状；三条基线拿到的是同一批视图。
fn variant(base: &Workload, base_rows: usize, cardinality: usize, views: usize) -> Workload {
    let mut w = base.clone();
    w.data.base_rows = base_rows;
    w.data.group_cardinality = cardinality;
    w.views = (0..views)
        .map(|i| ViewSpec {
            id: i,
            threshold: (i as i64 * 7) % 150,
        })
        .collect();
    w
}

fn run_one(
    base: &Workload,
    b: Baseline,
    rows: usize,
    card: usize,
    views: usize,
    batch: usize,
) -> rusqlite::Result<Record> {
    let mut w = variant(base, rows, card, views);
    w.updates.batch_size = batch;

    let conn = Connection::open_in_memory()?;

    // ---- 以下全部不计时：建立初始状态 ----
    seed_base(&conn, &w)?;
    if b == Baseline::HandWrittenTrigger {
        for v in &w.views {
            install_trigger_view(&conn, &w.schema.table, v)?;
        }
    }
    let ops = w.update_trace();

    // ---- 计时区间 ----
    let apply_ms = apply(&conn, &w.schema.table, &ops)?;
    let maintain_ms = match b {
        // trigger 的成本已计入 apply_ms——那正是写放大
        Baseline::NoMaintenance | Baseline::HandWrittenTrigger => 0.0,
        Baseline::NaiveRecompute => recompute_all(&conn, &w)?,
    };

    Ok(Record {
        baseline: b.label(),
        views,
        base_rows: rows,
        batch,
        cardinality: card,
        apply_ms,
        maintain_ms,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Workload::load(Path::new("workloads/m0-baseline.toml"))?;
    let mut records: Vec<Record> = Vec::new();

    let baselines = [
        Baseline::NoMaintenance,
        Baseline::HandWrittenTrigger,
        Baseline::NaiveRecompute,
    ];

    // 扫描一：group 基数 × 基表规模 × 批大小，视图数固定
    for b in baselines {
        for card in GROUP_CARDINALITIES {
            for rows in BASE_ROWS {
                // group 基数大于行数在语义上无意义——N 行的表不可能有多于 N 个
                // 不同的分组键。ivmlite-workload 在加载时就会拒绝这种配置，
                // 所以这里跳过而不是让它报错。被跳过的格子在 stderr 记一行，
                // 免得读 CSV 的人以为是漏跑了。
                if card > rows {
                    eprintln!("跳过无意义格子: card={card} > base_rows={rows}");
                    continue;
                }
                for batch in BATCH_SIZES {
                    records.push(run_one(&base, b, rows, card, FIXED_VIEWS, batch)?);
                }
            }
        }
    }

    // 扫描二：视图数 × 基表规模 × 批大小，group 基数固定
    for b in baselines {
        for views in VIEW_COUNTS {
            if views == FIXED_VIEWS {
                continue; // 与扫描一的共同点重复
            }
            for rows in BASE_ROWS {
                for batch in BATCH_SIZES {
                    records.push(run_one(&base, b, rows, FIXED_CARDINALITY, views, batch)?);
                }
            }
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
    // 自己挑了个好看的点（spec §10.1）。
    for card in GROUP_CARDINALITIES {
        let path = format!("docs/bench/m0-baseline-card{card}.svg");
        match plot::write_svg(Path::new(&path), &records, FIXED_VIEWS, 100, card) {
            Ok(()) => eprintln!("图已写入 {path}"),
            // 某个 group 基数在所有基表规模下都被跳过时没有数据点，
            // 这不是错误——照实说明并继续。
            Err(e) => eprintln!("跳过 card={card} 的出图: {e}"),
        }
    }

    Ok(())
}
