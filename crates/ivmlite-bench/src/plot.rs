use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::Record;

const W: f64 = 720.0;
const H: f64 = 420.0;
const PAD: f64 = 64.0;
const COLORS: [&str; 3] = ["#888888", "#1f77b4", "#d62728"];

/// 把 `10^log10_rows` 格式化成人类习惯的行数写法（`10k` / `1M`），
/// 用作横轴刻度标签。
fn fmt_rows(log10_rows: f64) -> String {
    let rows = 10f64.powf(log10_rows).round() as i64;
    if rows >= 1_000_000 && rows % 1_000_000 == 0 {
        format!("{}M", rows / 1_000_000)
    } else if rows >= 1_000 && rows % 1_000 == 0 {
        format!("{}k", rows / 1_000)
    } else {
        rows.to_string()
    }
}

/// 把毫秒数格式化成刻度标签，精度随量级调整，避免小数值全部显示成 `0.0`。
fn fmt_ms(ms: f64) -> String {
    if ms >= 100.0 {
        format!("{ms:.0}ms")
    } else if ms >= 1.0 {
        format!("{ms:.1}ms")
    } else if ms >= 0.01 {
        format!("{ms:.3}ms")
    } else {
        format!("{ms:.1e}ms")
    }
}

/// 画头条图：固定 views / batch / group 基数，横轴基表规模（对数），纵轴
/// 总耗时（对数），每条基线一条折线。交叉点就是两条线相交的地方（spec §10.4）。
///
/// group 基数必须固定并标在图上——交叉点随它剧烈移动，把不同基数的点混进
/// 同一张图会画出一条毫无意义的折线（spec §10.1）。
///
/// I11：纵轴此前是线性刻度。`naive_recompute` 与另外两条基线相差可达
/// 20000 倍时，`no_maintenance` 与 `hand_written_trigger` 会被压缩到同一
/// 条像素线上，图形回答不了任何读者会带来的问题。改成对数刻度，并给两根
/// 轴都加上刻度标签；某条基线在该基数下点数不足三个时（`card=100,000` 因
/// `card > base_rows` 被 `Workload::load` 拒绝而跳过 `base_rows=10,000`
/// 这一格），在图上显式标注，而不是让读者误以为漏跑。
pub fn write_svg(
    path: &Path,
    records: &[Record],
    fixed_views: usize,
    fixed_batch: usize,
    fixed_card: usize,
) -> std::io::Result<()> {
    let mut series: BTreeMap<&str, Vec<(f64, f64)>> = BTreeMap::new();
    for r in records {
        if r.views == fixed_views && r.batch == fixed_batch && r.cardinality == fixed_card {
            let total_ms = (r.apply_ms + r.maintain_ms).max(1e-6); // 对数刻度不能取 0 或负值
            series
                .entry(r.baseline)
                .or_default()
                .push(((r.base_rows as f64).log10(), total_ms.log10()));
        }
    }
    for pts in series.values_mut() {
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    let xs: Vec<f64> = series.values().flatten().map(|p| p.0).collect();
    let ys: Vec<f64> = series.values().flatten().map(|p| p.1).collect();
    if xs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("没有 views={fixed_views} batch={fixed_batch} card={fixed_card} 的数据点"),
        ));
    }
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::MAX, f64::min),
        xs.iter().cloned().fold(f64::MIN, f64::max),
    );
    // y 轴现在承载 log10(ms)；上下各留半格，刻度线落在整数 log10 上更好看。
    let (y0, y1) = (
        ys.iter().cloned().fold(f64::MAX, f64::min).floor() - 0.2,
        ys.iter().cloned().fold(f64::MIN, f64::max).ceil() + 0.2,
    );

    let sx = |x: f64| PAD + (x - x0) / (x1 - x0).max(1e-9) * (W - 2.0 * PAD);
    let sy = |y: f64| H - PAD - (y - y0) / (y1 - y0).max(1e-9) * (H - 2.0 * PAD);

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="sans-serif" font-size="12">
<rect width="{W}" height="{H}" fill="white"/>
<text x="{tx}" y="24" text-anchor="middle" font-size="15">apply + maintain &#183; views={fixed_views} &#183; batch={fixed_batch} &#183; groups={fixed_card}</text>
<line x1="{PAD}" y1="{by}" x2="{rx}" y2="{by}" stroke="#333"/>
<line x1="{PAD}" y1="{PAD}" x2="{PAD}" y2="{by}" stroke="#333"/>
<text x="{tx}" y="{lx}" text-anchor="middle">base_rows</text>
<text x="18" y="18" fill="#333">ms (log10)</text>
"##,
        tx = W / 2.0,
        by = H - PAD,
        rx = W - PAD,
        lx = H - 20.0,
    );

    // x 轴刻度：每个出现过的 base_rows 值画一条竖线 + 标签。
    let mut x_ticks: Vec<f64> = xs.clone();
    x_ticks.sort_by(|a, b| a.total_cmp(b));
    x_ticks.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    for x in &x_ticks {
        let px = sx(*x);
        svg.push_str(&format!(
            "<line x1=\"{px:.1}\" y1=\"{by:.1}\" x2=\"{px:.1}\" y2=\"{tick_end:.1}\" stroke=\"#333\"/>\n\
             <text x=\"{px:.1}\" y=\"{label_y:.1}\" text-anchor=\"middle\">{label}</text>\n",
            by = H - PAD,
            tick_end = H - PAD + 6.0,
            label_y = H - PAD + 18.0,
            label = fmt_rows(*x),
        ));
    }

    // y 轴刻度：每个整数 log10(ms) 一条横线 + 标签。
    let mut y_tick = y0.ceil();
    while y_tick <= y1 {
        let py = sy(y_tick);
        svg.push_str(&format!(
            "<line x1=\"{lx:.1}\" y1=\"{py:.1}\" x2=\"{PAD:.1}\" y2=\"{py:.1}\" stroke=\"#333\"/>\n\
             <text x=\"{label_x:.1}\" y=\"{py:.1}\" dy=\"4\" text-anchor=\"end\">{label}</text>\n",
            lx = PAD - 6.0,
            label_x = PAD - 10.0,
            label = fmt_ms(10f64.powf(y_tick)),
        ));
        y_tick += 1.0;
    }

    let mut annotations: Vec<String> = Vec::new();
    for (i, (name, pts)) in series.iter().enumerate() {
        let color = COLORS[i % COLORS.len()];
        let d: Vec<String> = pts
            .iter()
            .map(|(x, y)| format!("{:.1},{:.1}", sx(*x), sy(*y)))
            .collect();
        svg.push_str(&format!(
            "<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" points=\"{}\"/>\n",
            d.join(" ")
        ));
        for (x, y) in pts {
            svg.push_str(&format!(
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3\" fill=\"{color}\"/>\n",
                sx(*x),
                sy(*y)
            ));
        }
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{color}\">{name}</text>\n",
            W - PAD - 150.0,
            PAD + 18.0 * (i as f64 + 1.0)
        ));

        if pts.len() < 3 {
            annotations.push(format!(
                "{name} 只有 {n} 个数据点（其余格因 card={fixed_card} 超过某个 \
                 base_rows 被 Workload::load 拒绝而跳过)",
                n = pts.len()
            ));
        }
    }

    for (i, note) in annotations.iter().enumerate() {
        svg.push_str(&format!(
            "<text x=\"{PAD:.1}\" y=\"{y:.1}\" font-size=\"10\" fill=\"#a00\">{note}</text>\n",
            y = H - 8.0 - 12.0 * (annotations.len() - 1 - i) as f64,
        ));
    }

    svg.push_str("</svg>\n");

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(baseline: &'static str, base_rows: usize, apply_ms: f64, maintain_ms: f64) -> Record {
        rec_card(baseline, base_rows, apply_ms, maintain_ms, 1_000)
    }

    fn rec_card(
        baseline: &'static str,
        base_rows: usize,
        apply_ms: f64,
        maintain_ms: f64,
        cardinality: usize,
    ) -> Record {
        Record {
            baseline,
            views: 10,
            base_rows,
            batch: 100,
            cardinality,
            apply_ms,
            maintain_ms,
        }
    }

    /// 从生成的 SVG 里按 `fill` 颜色取出对应序列全部 `<circle>` 的 cy 坐标。
    fn circle_ys(svg: &str, color: &str) -> Vec<f64> {
        let needle = format!("fill=\"{color}\"");
        svg.lines()
            .filter(|l| l.contains("<circle") && l.contains(&needle))
            .map(|l| {
                let start = l.find("cy=\"").expect("circle 缺少 cy") + 4;
                let rest = &l[start..];
                let end = rest.find('"').expect("cy 属性未闭合");
                rest[..end].parse::<f64>().expect("cy 不是合法数字")
            })
            .collect()
    }

    /// I11 的核心守卫：纵轴必须是对数刻度。构造三条基线在同一个
    /// base_rows 下的总耗时——1ms / 100ms / 1,000,000ms——`no_maintenance`
    /// 与 `hand_written_trigger` 只差 100x，但 `naive_recompute` 把整个
    /// 值域拉到 1,000,000x。在线性刻度下，前两者会被压缩到同一条像素线上
    /// （相差不到 1 像素）；只有对数刻度能让它们保持几十像素的可辨间距。
    #[test]
    fn y_axis_uses_log_scale_not_linear() {
        let dir = std::env::temp_dir().join("ivmlite-bench-plot-log-scale");
        let path = dir.join("test.svg");
        let records = vec![
            rec("no_maintenance", 10_000, 1.0, 0.0),         // 总耗时 1ms
            rec("hand_written_trigger", 10_000, 100.0, 0.0), // 总耗时 100ms，100x
            rec("naive_recompute", 10_000, 0.0, 1_000_000.0), // 总耗时 1,000,000ms
        ];
        write_svg(&path, &records, 10, 100, 1_000).unwrap();
        let svg = fs::read_to_string(&path).unwrap();

        let no_maint_y = circle_ys(&svg, "#d62728")[0]; // no_maintenance 是第三个 key，取第 3 种颜色
        let trigger_y = circle_ys(&svg, "#888888")[0]; // hand_written_trigger 是第一个 key

        // 线性刻度下二者的像素间距 < 1；对数刻度下应有几十像素的可辨间距。
        assert!(
            (no_maint_y - trigger_y).abs() > 30.0,
            "no_maintenance(1ms) 与 hand_written_trigger(100ms) 的 y 坐标间距过小\
             （{no_maint_y:.2} vs {trigger_y:.2}）——这正是线性刻度会把小值\
             压缩到同一条像素线上的形状，说明纵轴没有真正用对数刻度"
        );
    }

    /// I11：两个轴都必须有刻度标签，不能只有一个 y1 数字。
    #[test]
    fn axes_have_tick_labels() {
        let dir = std::env::temp_dir().join("ivmlite-bench-plot-ticks");
        let path = dir.join("test.svg");
        let records = vec![
            rec("no_maintenance", 10_000, 1.0, 0.0),
            rec("no_maintenance", 100_000, 1.0, 0.0),
            rec("no_maintenance", 1_000_000, 1.0, 0.0),
        ];
        write_svg(&path, &records, 10, 100, 1_000).unwrap();
        let svg = fs::read_to_string(&path).unwrap();

        // x 轴：三个 base_rows 值都应有标签。
        assert!(
            svg.contains("10k"),
            "缺少 base_rows=10,000 的刻度标签: {svg}"
        );
        assert!(
            svg.contains("100k"),
            "缺少 base_rows=100,000 的刻度标签: {svg}"
        );
        assert!(
            svg.contains("1M"),
            "缺少 base_rows=1,000,000 的刻度标签: {svg}"
        );
        // y 轴：应至少有一个 ms 标签（不止过去那唯一的 y1 数字）。
        assert!(svg.contains("ms</text>"), "y 轴缺少刻度标签: {svg}");
    }

    /// I11：某条基线在该基数下点数不足三个时（`card=100,000` 因
    /// `card > base_rows` 跳过 `base_rows=10,000`），必须在图上显式标注。
    #[test]
    fn sparse_series_are_annotated() {
        let dir = std::env::temp_dir().join("ivmlite-bench-plot-sparse");
        let path = dir.join("test.svg");
        let records = vec![
            rec_card("no_maintenance", 100_000, 1.0, 0.0, 100_000),
            rec_card("no_maintenance", 1_000_000, 1.0, 0.0, 100_000),
        ];
        write_svg(&path, &records, 10, 100, 100_000).unwrap();
        let svg = fs::read_to_string(&path).unwrap();
        assert!(
            svg.contains("只有 2 个数据点"),
            "少于三个点的序列必须被标注: {svg}"
        );
    }

    #[test]
    fn fmt_rows_uses_k_and_m_suffixes() {
        assert_eq!(fmt_rows(4.0), "10k");
        assert_eq!(fmt_rows(5.0), "100k");
        assert_eq!(fmt_rows(6.0), "1M");
    }
}
