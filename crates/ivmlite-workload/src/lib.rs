use std::fs;
use std::path::Path;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

/// M0 has only Uniform. The enum exists already so that adding Zipf in M2 does
/// not change the workload file format (spec §10.6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Distribution {
    Uniform,
}

/// Likewise: M2 adds Hot (updates concentrated on hot groups).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Locality {
    Uniform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSchema {
    pub table: String,
    pub ddl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSpec {
    pub base_rows: usize,
    pub group_cardinality: usize,
    pub amount_max: i64,
    pub distribution: Distribution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSpec {
    pub batch_size: usize,
    pub delete_ratio: f64,
    pub locality: Locality,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    pub id: usize,
    pub threshold: i64,
}

/// The derivation rules for the M0 benchmark matrix (spec §10.3 item 7). This is
/// the single source of truth for `docs/bench/m0-baseline.csv`:
/// `Workload::cells()` reads this section and produces a concrete `Workload` for
/// every measured cell, replacing the hand-written constants and two sweep loops
/// that used to live in `ivmlite-bench/src/main.rs`. Any engine's runner that
/// loads the same workload file and calls `cells()` re-derives exactly the set
/// of cells this repository measured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixSpec {
    /// Base-table sizes shared by both sweeps.
    pub base_rows: Vec<usize>,
    /// Batch sizes shared by both sweeps.
    pub batch_sizes: Vec<usize>,
    /// View counts for sweep two (view count × base-table size × batch size).
    pub view_counts: Vec<usize>,
    /// Group cardinalities for sweep one (group cardinality × base-table size × batch size).
    pub group_cardinalities: Vec<usize>,
    /// Sweep one's fixed view count.
    pub fixed_views: usize,
    /// Sweep two's fixed group cardinality.
    pub fixed_cardinality: usize,
    /// The threshold of view i = `(i * view_threshold_stride) % view_threshold_modulus`.
    pub view_threshold_stride: i64,
    pub view_threshold_modulus: i64,
}

impl MatrixSpec {
    /// Derive the set of `n` views. The threshold formula is what keeps the views
    /// distinct from one another; it is exactly the formula the benchmark runner
    /// hard-coded before `[matrix]` existed, so the committed CSV stays
    /// reproducible (see the `[matrix]` comments in `workloads/m0-baseline.toml`).
    fn views(&self, n: usize) -> Vec<ViewSpec> {
        (0..n)
            .map(|i| ViewSpec {
                id: i,
                threshold: (i as i64 * self.view_threshold_stride) % self.view_threshold_modulus,
            })
            .collect()
    }
}

impl ViewSpec {
    pub fn sql(&self, table: &str) -> String {
        format!(
            "SELECT region, SUM(amount), COUNT(*) FROM \"{}\" \
             WHERE amount > {} GROUP BY region",
            table, self.threshold
        )
    }

    pub fn table(&self) -> String {
        format!("mv_{}", self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workload {
    pub name: String,
    pub seed: u64,
    pub schema: WorkloadSchema,
    pub data: DataSpec,
    pub updates: UpdateSpec,
    pub views: Vec<ViewSpec>,
    /// The derivation rules for the M0 benchmark matrix (spec §10.3 item 7). Only
    /// a workload file that is the starting point of a matrix (such as
    /// `workloads/m0-baseline.toml`) needs this section; in the concrete cells
    /// `cells()` produces this field is `None` — they are already the result of
    /// evaluating the matrix and need no evaluation rules of their own.
    /// `#[serde(default)]` lets workload files without a `[matrix]` section (and
    /// test code that builds `Workload` literals) keep leaving it out.
    #[serde(default)]
    pub matrix: Option<MatrixSpec>,
    /// The fixed set of cells `ivmlite-bench ablation` measures (M1b Phase 4
    /// Task 2, spec §6). Like `matrix`, only the workload file that is the
    /// ablation's starting point (`workloads/m0-baseline.toml`) needs this
    /// section; `#[serde(default)]` keeps every other workload file and test
    /// literal from needing one.
    #[serde(default)]
    pub ablation: Option<AblationSpec>,
}

/// One row of spec §6's ablation table: a representative combination of the
/// same four dimensions `cells()` sweeps, held fixed rather than derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AblationCell {
    pub base_rows: usize,
    pub group_cardinality: usize,
    pub views: usize,
    pub batch_size: usize,
}

/// Spec §6's ablation: a fixed set of cells, each repeated `repeats` times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AblationSpec {
    pub cells: Vec<AblationCell>,
    pub repeats: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceOp {
    Insert {
        id: i64,
        region: String,
        amount: i64,
    },
    Delete {
        id: i64,
    },
}

#[derive(Debug)]
pub enum WorkloadError {
    Io(std::io::Error),
    Parse(String),
    Invalid(String),
}

impl std::fmt::Display for WorkloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkloadError::Io(e) => write!(f, "{e}"),
            WorkloadError::Parse(e) => write!(f, "failed to parse the workload: {e}"),
            WorkloadError::Invalid(e) => write!(f, "invalid workload configuration: {e}"),
        }
    }
}

impl std::error::Error for WorkloadError {}

impl Workload {
    pub fn load(path: &Path) -> Result<Workload, WorkloadError> {
        let text = fs::read_to_string(path).map_err(WorkloadError::Io)?;
        let w: Workload = toml::from_str(&text).map_err(|e| WorkloadError::Parse(e.to_string()))?;
        w.validate()?;
        Ok(w)
    }

    /// `group_cardinality > base_rows` is meaningless: a table of N rows cannot
    /// hold more than N distinct group keys. Better to reject it at load time
    /// than to quietly generate fewer — otherwise the benchmark would report
    /// results under a cardinality it never actually used.
    ///
    /// `pub` (rather than private to `load`) because this rule should have
    /// exactly one home: `cells()` calls this method directly to decide whether
    /// a matrix cell is legal, so the rule is not maintained twice in two
    /// spellings that would sooner or later diverge.
    pub fn validate(&self) -> Result<(), WorkloadError> {
        if self.data.group_cardinality > self.data.base_rows {
            return Err(WorkloadError::Invalid(format!(
                "group_cardinality ({}) must not exceed base_rows ({})",
                self.data.group_cardinality, self.data.base_rows
            )));
        }
        Ok(())
    }

    /// The base-table rows. Ids are dense and unique; the number of distinct
    /// group keys is **exactly** group_cardinality — the first card rows cover
    /// each key in turn, and the remaining rows fall into existing keys at
    /// random. Random placement alone cannot guarantee every key is covered, and
    /// the benchmark depends on this number being exact.
    ///
    /// Precondition: `base_rows >= group_cardinality`. `validate` (which `load`
    /// calls) enforces it — the reverse (more group keys than rows) is
    /// meaningless, since a table of N rows cannot hold more than N distinct
    /// keys. A caller that builds a `Workload` directly (without `load`) must
    /// uphold it itself, or the number of group keys quietly degrades to
    /// `base_rows`.
    pub fn rows(&self) -> impl Iterator<Item = (i64, String, i64)> + '_ {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let card = self.data.group_cardinality.max(1);
        let amount_max = self.data.amount_max.max(1);
        (0..self.data.base_rows).map(move |i| {
            let g = if i < card {
                i
            } else {
                rng.random_range(0..card)
            };
            (i as i64, format!("r{g}"), rng.random_range(0..amount_max))
        })
    }

    /// One batch of updates. Every DELETE hits an id that exists and has not
    /// been deleted yet, and every INSERT uses a fresh id, so the trace is
    /// always legal: any runner can replay it directly, without keeping its own
    /// model of "which rows are still alive".
    pub fn update_trace(&self) -> Vec<TraceOp> {
        let mut rng = StdRng::seed_from_u64(self.seed ^ 0x5EED);
        let card = self.data.group_cardinality.max(1);
        let amount_max = self.data.amount_max.max(1);
        let base = self.data.base_rows as i64;
        let mut next_id = base;
        let mut deleted: std::collections::BTreeSet<i64> = Default::default();

        (0..self.updates.batch_size)
            .map(|_| {
                let want_delete = rng.random_bool(self.updates.delete_ratio.clamp(0.0, 1.0));
                if want_delete && (deleted.len() as i64) < base {
                    let mut id = rng.random_range(0..base);
                    while deleted.contains(&id) {
                        id = rng.random_range(0..base);
                    }
                    deleted.insert(id);
                    TraceOp::Delete { id }
                } else {
                    let id = next_id;
                    next_id += 1;
                    TraceOp::Insert {
                        id,
                        region: format!("r{}", rng.random_range(0..card)),
                        amount: rng.random_range(0..amount_max),
                    }
                }
            })
            .collect()
    }

    /// Export in a form any engine can load: schema.sql / views.sql / data.csv /
    /// updates.csv. This is how the "workloads are portable" constraint is
    /// actually met (spec §10.3 item 7).
    pub fn export(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("schema.sql"), format!("{};\n", self.schema.ddl))?;

        let views: String = self
            .views
            .iter()
            .map(|v| format!("-- {}\n{};\n", v.table(), v.sql(&self.schema.table)))
            .collect();
        fs::write(dir.join("views.sql"), views)?;

        let mut data = String::from("id,region,amount\n");
        for (id, region, amount) in self.rows() {
            data.push_str(&format!("{id},{region},{amount}\n"));
        }
        fs::write(dir.join("data.csv"), data)?;

        let mut ups = String::from("op,id,region,amount\n");
        for op in self.update_trace() {
            match op {
                TraceOp::Insert { id, region, amount } => {
                    ups.push_str(&format!("insert,{id},{region},{amount}\n"))
                }
                TraceOp::Delete { id } => ups.push_str(&format!("delete,{id},,\n")),
            }
        }
        fs::write(dir.join("updates.csv"), ups)
    }

    /// Expand the `[matrix]` section into every concrete cell the M0 benchmark
    /// matrix measures (spec §10.3 item 7): the rows of
    /// `docs/bench/m0-baseline.csv` are the three baselines each run once over
    /// the set of cells returned here. This moves the rule "how the measured
    /// matrix is derived from one benchmark workload" into `ivmlite-workload`:
    /// any engine's runner that loads the same workload file and calls this
    /// method re-derives exactly the cells this repository measured, without
    /// re-implementing the two-sweep structure, the view-threshold formula, or
    /// the skip rule below.
    ///
    /// It keeps the two-sweep structure **exactly** (not a full four-way cross,
    /// which would be 144 cells — too many, spec §10.1):
    /// - Sweep one: group cardinality × base-table size × batch size, with the
    ///   view count fixed at `matrix.fixed_views`.
    /// - Sweep two: view count × base-table size × batch size, with the group
    ///   cardinality fixed at `matrix.fixed_cardinality`; view counts equal to
    ///   `fixed_views` are skipped, so the point `(fixed_views,
    ///   fixed_cardinality)` shared with sweep one is not measured twice.
    ///
    /// Skip rule: combinations with `group_cardinality > base_rows` are not
    /// emitted — a table of N rows cannot hold more than N distinct group keys.
    /// This is **the same rule** by which `validate()` rejects the combination,
    /// and its only home is `validate()`; here we merely avoid generating
    /// illegal cells rather than re-deciding legality.
    ///
    /// # Panics
    /// If `self.matrix` is `None` (this workload is not the starting point of a
    /// matrix), or if some derived cell fails `validate()` (meaning `[matrix]`
    /// itself is inconsistent).
    pub fn cells(&self) -> Vec<Workload> {
        let m = self
            .matrix
            .as_ref()
            .expect("cells() needs a [matrix] section in the workload");

        let mut cells = Vec::new();

        // Sweep one: group cardinality × base-table size × batch size, fixed view count.
        for &card in &m.group_cardinalities {
            for &rows in &m.base_rows {
                if card > rows {
                    continue;
                }
                for &batch in &m.batch_sizes {
                    cells.push(self.with_cell(rows, card, m.fixed_views, batch));
                }
            }
        }

        // Sweep two: view count × base-table size × batch size, fixed group cardinality.
        for &views in &m.view_counts {
            if views == m.fixed_views {
                continue; // Already measured as sweep one's shared point.
            }
            for &rows in &m.base_rows {
                if m.fixed_cardinality > rows {
                    continue;
                }
                for &batch in &m.batch_sizes {
                    cells.push(self.with_cell(rows, m.fixed_cardinality, views, batch));
                }
            }
        }

        cells
    }

    /// Build one concrete cell: clone self, change the four dimensions
    /// `base_rows` / `group_cardinality` / `batch_size` / `views` (the latter
    /// expanded through the matrix's view-threshold formula), and call
    /// `validate()` explicitly before returning — building by clone + field
    /// mutation bypasses the `validate()` inside `load()`, so it is called again
    /// here, keeping `validate()` the only place the `group_cardinality >
    /// base_rows` rule is enforced. A derived cell is already a concrete
    /// configuration with the matrix evaluated, so its `matrix` field is cleared
    /// to `None` — it needs no evaluation rules of its own.
    ///
    /// `cells()` calls this for every point of the M0 matrix, and Task 2's
    /// `confirm` and `ablation` modes call it for cells selected or fixed
    /// outside the matrix — the two can never disagree on how a cell is built
    /// from its four dimensions, because there is only one implementation.
    ///
    /// # Panics
    /// If `self.matrix` is `None` (the view-threshold formula lives there), or
    /// if the resulting cell fails `validate()`.
    pub fn with_cell(
        &self,
        base_rows: usize,
        group_cardinality: usize,
        views: usize,
        batch_size: usize,
    ) -> Workload {
        let m = self
            .matrix
            .as_ref()
            .expect("with_cell needs a [matrix] section in the workload");
        let mut w = self.clone();
        w.data.base_rows = base_rows;
        w.data.group_cardinality = group_cardinality;
        w.updates.batch_size = batch_size;
        w.views = m.views(views);
        w.matrix = None;
        w.validate()
            .unwrap_or_else(|e| panic!("with_cell built an invalid workload: {e}"));
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn spec() -> Workload {
        Workload {
            name: "t".into(),
            seed: 1,
            schema: WorkloadSchema {
                table: "orders".into(),
                ddl: "CREATE TABLE orders(id INTEGER PRIMARY KEY, region TEXT NOT NULL, amount INTEGER NOT NULL) STRICT".into(),
            },
            data: DataSpec {
                base_rows: 500,
                group_cardinality: 7,
                amount_max: 50,
                distribution: Distribution::Uniform,
            },
            updates: UpdateSpec {
                batch_size: 30,
                delete_ratio: 0.5,
                locality: Locality::Uniform,
            },
            views: vec![
                ViewSpec { id: 0, threshold: 0 },
                ViewSpec { id: 1, threshold: 10 },
            ],
            matrix: None,
            ablation: None,
        }
    }

    fn matrix_spec() -> MatrixSpec {
        MatrixSpec {
            base_rows: vec![10, 100, 1000],
            batch_sizes: vec![1, 10],
            view_counts: vec![1, 3, 5],
            group_cardinalities: vec![5, 50, 5000],
            fixed_views: 3,
            fixed_cardinality: 50,
            view_threshold_stride: 7,
            view_threshold_modulus: 150,
        }
    }

    fn spec_with_matrix() -> Workload {
        let mut w = spec();
        w.matrix = Some(matrix_spec());
        w
    }

    #[test]
    fn rows_respect_group_cardinality() {
        let regions: BTreeSet<String> = spec().rows().map(|(_, r, _)| r).collect();
        assert_eq!(
            regions.len(),
            7,
            "the number of distinct group keys must be exactly group_cardinality — it is the benchmark's core dimension"
        );
    }

    /// A gap test (added by M1a Phase 1 Task 1's mutation audit):
    /// `rows_respect_group_cardinality` runs 500 rows over 7 group keys, and
    /// purely random assignment almost certainly covers all 7 keys too — when
    /// the mutation that changes `rows()` from "the first card rows cover each
    /// key in turn" to "every row is placed purely at random" was actually run,
    /// that test stayed green. What did catch the mutation was
    /// `base_rows_equal_to_group_cardinality_is_accepted`, but that test's name
    /// and intent are "the boundary value is accepted", not "group-key coverage
    /// is exact"; it caught the mutation only as a side effect of the scenario's
    /// small sample (7 draws covering 7 keys is unlikely).
    ///
    /// This pins the mechanism the doc comment claims ("the first card rows
    /// cover each key in turn") as an assertion: instead of counting the final
    /// distinct values, it checks that the first `card` rows' group keys appear
    /// exactly in `0..card` order. Purely random assignment is very unlikely to
    /// produce that order by chance.
    #[test]
    fn first_card_rows_deterministically_cover_each_group_in_order() {
        let w = spec();
        let card = w.data.group_cardinality;
        let regions: Vec<String> = w.rows().take(card).map(|(_, r, _)| r).collect();
        let want: Vec<String> = (0..card).map(|i| format!("r{i}")).collect();
        assert_eq!(
            regions, want,
            "the first group_cardinality rows must cover each group key once, in order \
             (spec §10.1) — this is the mechanism that makes coverage exact, rather than \
             relying on later random sampling to probably fill every key"
        );
    }

    #[test]
    fn row_ids_are_dense_and_unique() {
        let ids: Vec<i64> = spec().rows().map(|(id, _, _)| id).collect();
        assert_eq!(ids.len(), 500);
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 500);
        assert_eq!(*ids.iter().min().unwrap(), 0);
        assert_eq!(*ids.iter().max().unwrap(), 499);
    }

    #[test]
    fn trace_is_always_legal() {
        let w = spec();
        let mut live: BTreeSet<i64> = w.rows().map(|(id, _, _)| id).collect();
        for op in w.update_trace() {
            match op {
                TraceOp::Insert { id, .. } => {
                    assert!(
                        live.insert(id),
                        "the trace must not insert the same id twice"
                    );
                }
                TraceOp::Delete { id } => {
                    assert!(
                        live.remove(&id),
                        "every DELETE in the trace must hit an existing id"
                    );
                }
            }
        }
    }

    #[test]
    fn view_sql_matches_threshold() {
        let sql = spec().views[1].sql("orders");
        assert!(sql.contains("amount > 10"), "{sql}");
        assert!(sql.contains("GROUP BY region"), "{sql}");
    }

    #[test]
    fn same_seed_yields_same_trace() {
        assert_eq!(spec().update_trace(), spec().update_trace());
    }

    #[test]
    fn shipped_workload_file_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../workloads/m0-baseline.toml");
        let w = Workload::load(&path).expect("the shipped workload file must parse");
        assert_eq!(w.name, "m0-baseline");
        assert!(!w.views.is_empty());
        assert!(
            w.schema.ddl.contains("INTEGER PRIMARY KEY"),
            "spec §10.3 item 5 requires a stable primary key"
        );
    }

    #[test]
    fn exported_artifacts_load_into_sqlite() {
        let dir = std::env::temp_dir().join("ivmlite-workload-export");
        let mut w = spec();
        w.data.base_rows = 50;
        w.updates.batch_size = 10;
        w.export(&dir).unwrap();

        for f in ["schema.sql", "views.sql", "data.csv", "updates.csv"] {
            assert!(dir.join(f).exists(), "missing export artifact {f}");
        }
        let data = std::fs::read_to_string(dir.join("data.csv")).unwrap();
        assert_eq!(data.lines().count(), 51, "header + 50 rows");
    }

    /// `cells()` derives each configuration by cloning and mutating a base
    /// workload; the clone must be independent of the original, so changing one
    /// cannot touch the other.
    #[test]
    fn workload_clones_independently_of_the_original() {
        let original = spec();
        let mut variant = original.clone();
        variant.name = "variant".into();
        variant.data.base_rows = 999;
        variant.views.push(ViewSpec {
            id: 2,
            threshold: 99,
        });

        assert_eq!(original.name, "t");
        assert_eq!(original.data.base_rows, 500);
        assert_eq!(original.views.len(), 2);
    }

    /// `validate` must be callable directly, without going through `load` (that
    /// is, without writing TOML to disk and parsing it back) — the precondition
    /// for `cells()` reusing it instead of maintaining its own copy of the same
    /// rule.
    #[test]
    fn validate_is_directly_callable_without_going_through_load() {
        let mut w = spec();
        w.data.base_rows = 5;
        w.data.group_cardinality = 50;
        let err = w.validate().expect_err("card > base_rows must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("50") && msg.contains('5'));

        w.data.group_cardinality = 5;
        assert!(
            w.validate().is_ok(),
            "card == base_rows is a legal boundary"
        );
    }

    /// group_cardinality > base_rows is meaningless (a table of N rows cannot
    /// hold more than N group keys); load must reject it at load time rather
    /// than quietly generate fewer group keys.
    #[test]
    fn load_rejects_group_cardinality_exceeding_base_rows() {
        let mut w = spec();
        w.data.base_rows = 10;
        w.data.group_cardinality = 100;
        let toml_text = toml::to_string(&w).unwrap();

        let dir = std::env::temp_dir().join("ivmlite-workload-invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, toml_text).unwrap();

        let err =
            Workload::load(&path).expect_err("group_cardinality > base_rows must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("100"), "{msg}");
        assert!(msg.contains("10"), "{msg}");
    }

    /// base_rows == group_cardinality is a legal boundary (every row its own
    /// group); an off-by-one in the comparison would reject this boundary too,
    /// so assert separately that it is accepted and the group-key count is exact.
    #[test]
    fn base_rows_equal_to_group_cardinality_is_accepted() {
        let mut w = spec();
        w.data.base_rows = 7;
        w.data.group_cardinality = 7;
        let toml_text = toml::to_string(&w).unwrap();

        let dir = std::env::temp_dir().join("ivmlite-workload-boundary");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boundary.toml");
        std::fs::write(&path, toml_text).unwrap();

        let loaded =
            Workload::load(&path).expect("base_rows == group_cardinality must be accepted");
        let regions: BTreeSet<String> = loaded.rows().map(|(_, r, _)| r).collect();
        assert_eq!(regions.len(), 7);
    }

    /// `cells()` must be deterministic — the same input gives the same output on
    /// every call. This is both a direct requirement of "the file fully
    /// determines the set of cells" (spec §10.3 item 7) and the precondition for
    /// the CSV-consistency test: if two calls could give different results, the
    /// statement "the set `cells()` produces equals the set in the CSV" would
    /// mean nothing.
    #[test]
    fn cells_is_deterministic() {
        let w = spec_with_matrix();
        let a = w.cells();
        let b = w.cells();

        let key = |c: &Workload| {
            (
                c.views.len(),
                c.data.base_rows,
                c.updates.batch_size,
                c.data.group_cardinality,
            )
        };
        assert_eq!(a.len(), b.len());
        assert_eq!(
            a.iter().map(key).collect::<Vec<_>>(),
            b.iter().map(key).collect::<Vec<_>>(),
            "two calls to cells() must give exactly the same sequence of cells"
        );
    }

    /// Every cell `cells()` derives must be a legal workload: clone + field
    /// mutation bypasses the `validate()` inside `load()`, so `cell()` calls it
    /// again — this test asserts that invariant directly rather than trusting
    /// the comment in the implementation.
    #[test]
    fn every_cell_passes_validate() {
        let w = spec_with_matrix();
        for c in w.cells() {
            c.validate()
                .unwrap_or_else(|e| panic!("cells() produced a cell that fails validate(): {e}"));
        }
    }

    /// A direct assertion of the skip rule: no emitted cell has
    /// `group_cardinality > base_rows`. `matrix_spec()`'s `group_cardinalities`
    /// deliberately includes 5000 — larger than every `base_rows` value — to
    /// trigger the rule.
    #[test]
    fn no_emitted_cell_has_cardinality_exceeding_base_rows() {
        let w = spec_with_matrix();
        for c in w.cells() {
            assert!(
                c.data.group_cardinality <= c.data.base_rows,
                "cells() should not emit a cell with group_cardinality={} > base_rows={}",
                c.data.group_cardinality,
                c.data.base_rows
            );
        }
    }

    /// Task 2's requirement in its own words: `with_cell` must be able to
    /// reproduce every cell `cells()` returns, called with exactly that cell's
    /// own four dimensions — so `cells()` and `with_cell` (which `confirm` and
    /// `ablation` call directly) can never disagree, because `cells()` is
    /// implemented in terms of `with_cell` rather than a second copy of the
    /// same derivation.
    #[test]
    fn with_cell_reproduces_every_cell_cells_returns() {
        let w = spec_with_matrix();
        for c in w.cells() {
            let reproduced = w.with_cell(
                c.data.base_rows,
                c.data.group_cardinality,
                c.views.len(),
                c.updates.batch_size,
            );
            assert_eq!(reproduced.data.base_rows, c.data.base_rows);
            assert_eq!(reproduced.data.group_cardinality, c.data.group_cardinality);
            assert_eq!(reproduced.updates.batch_size, c.updates.batch_size);
            assert_eq!(reproduced.views, c.views);
            assert!(reproduced.matrix.is_none());
        }
    }

    /// The shipped `workloads/m0-baseline.toml` gains a `[ablation]` section
    /// (Task 2, spec §6 / §8): exactly the four cells of the spec's table, with
    /// `repeats = 5`. This is the only test that pins the table's actual
    /// numbers against the committed file.
    #[test]
    fn shipped_workload_file_has_the_spec_6_ablation_section() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../workloads/m0-baseline.toml");
        let w = Workload::load(&path).expect("the shipped workload file must parse");
        let ablation = w
            .ablation
            .expect("workloads/m0-baseline.toml must have an [ablation] section");
        assert_eq!(ablation.repeats, 5);
        assert_eq!(
            ablation.cells,
            vec![
                AblationCell {
                    base_rows: 100_000,
                    group_cardinality: 1_000,
                    views: 10,
                    batch_size: 1_000
                },
                AblationCell {
                    base_rows: 100_000,
                    group_cardinality: 100_000,
                    views: 10,
                    batch_size: 1_000
                },
                AblationCell {
                    base_rows: 100_000,
                    group_cardinality: 1_000,
                    views: 200,
                    batch_size: 1_000
                },
                AblationCell {
                    base_rows: 100_000,
                    group_cardinality: 100_000,
                    views: 200,
                    batch_size: 100
                },
            ]
        );
    }

    /// The skip rule really takes effect: compared with the size of the naive
    /// cross product of both sweeps computed without any skipping, the number
    /// of cells `cells()` actually produces must be smaller. In `matrix_spec()`,
    /// `group_cardinalities` contains 5000 (always larger than every
    /// `base_rows`), and `fixed_cardinality=50` does not fit `base_rows=10`
    /// either, so both paths trigger a skip.
    #[test]
    fn skip_rule_emits_fewer_cells_than_naive_cross_product() {
        let w = spec_with_matrix();
        let m = w.matrix.as_ref().unwrap();

        // Sweep one's naive cross product: no card > rows check.
        let naive_scan_one = m.group_cardinalities.len() * m.base_rows.len() * m.batch_sizes.len();
        // Sweep two's naive cross product: still excludes the fixed_views point
        // shared with sweep one (that is the two-sweep structure itself, not the
        // skip rule), but has no card > rows check.
        let naive_scan_two = (m.view_counts.len() - 1) * m.base_rows.len() * m.batch_sizes.len();
        let naive_total = naive_scan_one + naive_scan_two;

        let actual = w.cells().len();
        assert!(
            actual < naive_total,
            "the skip rule should make cells() produce fewer cells ({actual}) than the naive \
             cross product ({naive_total})"
        );
    }
}
