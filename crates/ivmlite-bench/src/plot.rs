use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::Record;

const W: f64 = 720.0;
const H: f64 = 420.0;
const PAD: f64 = 64.0;
const COLORS: [&str; 3] = ["#888888", "#1f77b4", "#d62728"];

/// Format `10^log10_rows` the way people write row counts (`10k` / `1M`), for
/// the x-axis tick labels.
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

/// Format milliseconds as a tick label, scaling precision with magnitude so
/// small values do not all collapse to `0.0`.
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

/// Draw the headline chart: views, batch size and group cardinality fixed;
/// base-table size on the x axis (log), total time on the y axis (log); one
/// line per baseline. The crossover is where two lines meet (spec §10.4).
///
/// Group cardinality must be fixed and printed on the chart: the crossover
/// moves sharply with it, and mixing points from different cardinalities on
/// one chart draws a meaningless line (spec §10.1).
///
/// I11: the y axis used to be linear. With `naive_recompute` up to 20,000x
/// away from the other two baselines, `no_maintenance` and
/// `hand_written_trigger` collapsed onto the same pixel row, and the chart
/// answered none of the questions a reader brings to it. It is now log-scaled,
/// both axes carry tick labels, and a baseline with fewer than three points at
/// this cardinality is annotated on the chart — `card=100,000` skips the
/// `base_rows=10,000` cell because `Workload::load` rejects `card > base_rows` —
/// so a reader does not mistake the gap for a run that was never done.
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
            let total_ms = (r.apply_ms + r.maintain_ms).max(1e-6); // a log scale cannot take 0 or negatives
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
            format!("no data points for views={fixed_views} batch={fixed_batch} card={fixed_card}"),
        ));
    }
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::MAX, f64::min),
        xs.iter().cloned().fold(f64::MIN, f64::max),
    );
    // The y axis carries log10(ms). Pad a fraction of a decade on each side so
    // the tick lines fall on whole log10 values.
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

    // X-axis ticks: one mark and label per base_rows value present.
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

    // Y-axis ticks: one mark and label per whole log10(ms).
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
                "{name} has only {n} data points (the other cells were skipped because \
                 card={fixed_card} exceeds their base_rows, which Workload::load rejects)",
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

    /// Extract the `cy` coordinate of every `<circle>` drawn in `color`.
    fn circle_ys(svg: &str, color: &str) -> Vec<f64> {
        let needle = format!("fill=\"{color}\"");
        svg.lines()
            .filter(|l| l.contains("<circle") && l.contains(&needle))
            .map(|l| {
                let start = l.find("cy=\"").expect("circle has no cy") + 4;
                let rest = &l[start..];
                let end = rest.find('"').expect("unterminated cy attribute");
                rest[..end].parse::<f64>().expect("cy is not a number")
            })
            .collect()
    }

    /// The core guard for I11: the y axis must be log-scaled. Three baselines
    /// at one base_rows total 1ms / 100ms / 1,000,000ms. `no_maintenance` and
    /// `hand_written_trigger` differ by only 100x, but `naive_recompute`
    /// stretches the range to 1,000,000x. On a linear scale the first two land
    /// less than a pixel apart; only a log scale keeps tens of pixels between them.
    #[test]
    fn y_axis_uses_log_scale_not_linear() {
        let dir = std::env::temp_dir().join("ivmlite-bench-plot-log-scale");
        let path = dir.join("test.svg");
        let records = vec![
            rec("no_maintenance", 10_000, 1.0, 0.0),          // total 1ms
            rec("hand_written_trigger", 10_000, 100.0, 0.0),  // total 100ms, 100x
            rec("naive_recompute", 10_000, 0.0, 1_000_000.0), // total 1,000,000ms
        ];
        write_svg(&path, &records, 10, 100, 1_000).unwrap();
        let svg = fs::read_to_string(&path).unwrap();

        let no_maint_y = circle_ys(&svg, "#d62728")[0]; // no_maintenance is the third key, so the third color
        let trigger_y = circle_ys(&svg, "#888888")[0]; // hand_written_trigger is the first key

        // Under a linear scale these are under a pixel apart; under a log scale
        // they should be tens of pixels apart.
        assert!(
            (no_maint_y - trigger_y).abs() > 30.0,
            "no_maintenance (1ms) and hand_written_trigger (100ms) are too close \
             ({no_maint_y:.2} vs {trigger_y:.2}) — the shape a linear scale produces \
             when it squeezes small values onto one pixel row, so the y axis is not \
             actually log-scaled"
        );
    }

    /// I11: both axes must carry tick labels, not just a single y1 number.
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

        // X axis: each of the three base_rows values needs a label.
        assert!(
            svg.contains("10k"),
            "missing the tick label for base_rows=10,000: {svg}"
        );
        assert!(
            svg.contains("100k"),
            "missing the tick label for base_rows=100,000: {svg}"
        );
        assert!(
            svg.contains("1M"),
            "missing the tick label for base_rows=1,000,000: {svg}"
        );
        // Y axis: at least one ms label, beyond the single y1 number of old.
        assert!(
            svg.contains("ms</text>"),
            "the y axis has no tick labels: {svg}"
        );
    }

    /// I11: a baseline with fewer than three points at this cardinality
    /// (`card=100,000` skips `base_rows=10,000` because `card > base_rows`)
    /// must be annotated on the chart.
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
            svg.contains("has only 2 data points"),
            "a series with fewer than three points must be annotated: {svg}"
        );
    }

    #[test]
    fn fmt_rows_uses_k_and_m_suffixes() {
        assert_eq!(fmt_rows(4.0), "10k");
        assert_eq!(fmt_rows(5.0), "100k");
        assert_eq!(fmt_rows(6.0), "1M");
    }
}
