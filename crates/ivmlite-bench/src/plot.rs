use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::Record;

const W: f64 = 720.0;
const H: f64 = 420.0;
const PAD: f64 = 64.0;
const COLORS: [&str; 3] = ["#888888", "#1f77b4", "#d62728"];

/// 画头条图：固定 views / batch / group 基数，横轴基表规模（对数），纵轴
/// 总耗时，每条基线一条折线。交叉点就是两条线相交的地方（spec §10.4）。
///
/// group 基数必须固定并标在图上——交叉点随它剧烈移动，把不同基数的点混进
/// 同一张图会画出一条毫无意义的折线（spec §10.1）。
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
            series
                .entry(r.baseline)
                .or_default()
                .push(((r.base_rows as f64).log10(), r.apply_ms + r.maintain_ms));
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
    let y1 = ys.iter().cloned().fold(f64::MIN, f64::max).max(1e-6);

    let sx = |x: f64| PAD + (x - x0) / (x1 - x0).max(1e-9) * (W - 2.0 * PAD);
    let sy = |y: f64| H - PAD - (y / y1) * (H - 2.0 * PAD);

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="sans-serif" font-size="12">
<rect width="{W}" height="{H}" fill="white"/>
<text x="{tx}" y="24" text-anchor="middle" font-size="15">apply + maintain &#183; views={fixed_views} &#183; batch={fixed_batch} &#183; groups={fixed_card}</text>
<line x1="{PAD}" y1="{by}" x2="{rx}" y2="{by}" stroke="#333"/>
<line x1="{PAD}" y1="{PAD}" x2="{PAD}" y2="{by}" stroke="#333"/>
<text x="{tx}" y="{lx}" text-anchor="middle">base_rows (log10)</text>
<text x="16" y="{PAD}" fill="#333">{y1:.1} ms</text>
"##,
        tx = W / 2.0,
        by = H - PAD,
        rx = W - PAD,
        lx = H - 20.0,
    );

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
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{color}\">{name}</text>\n",
            W - PAD - 150.0,
            PAD + 18.0 * (i as f64 + 1.0)
        ));
    }
    svg.push_str("</svg>\n");

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, svg)
}
