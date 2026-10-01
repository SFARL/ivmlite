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

/// M1b Phase 4 Task 5 (spec §7): the write-amplification workload. Separate
/// from `Workload` because it needs REPLACE, UPDATE and `recursive_triggers`,
/// none of which the m0-shaped matrix (`Workload`) uses, and its own schema
/// (`accounts`) has two ordinary UNIQUE keys besides the `INTEGER PRIMARY
/// KEY` — §3's matrix inserts and deletes only through the primary key, so
/// it cannot measure what REPLACE's UNIQUE-candidate lookup costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOpKind {
    Insert,
    Delete,
    Update,
    ReplaceRowid,
    ReplaceUnique,
    ReplaceTwo,
}

impl WriteOpKind {
    /// The label written to the write-amp CSV's `op` column and read back
    /// from `[[ops]]` in the workload file — the same spelling
    /// `#[serde(rename_all = "snake_case")]` already produces, spelled out
    /// explicitly so `ivmlite-bench` does not need to round-trip through
    /// `serde` just to print a column.
    pub fn label(self) -> &'static str {
        match self {
            WriteOpKind::Insert => "insert",
            WriteOpKind::Delete => "delete",
            WriteOpKind::Update => "update",
            WriteOpKind::ReplaceRowid => "replace_rowid",
            WriteOpKind::ReplaceUnique => "replace_unique",
            WriteOpKind::ReplaceTwo => "replace_two",
        }
    }

    /// A distinct RNG seed offset per kind, so `WriteAmpWorkload::trace`'s
    /// per-kind trace is reproducible on its own (each kind's trace is
    /// generated independently, against a freshly seeded table) without two
    /// kinds ever drawing from the same random sequence.
    fn seed_salt(self) -> u64 {
        match self {
            WriteOpKind::Insert => 0x1A5E_9F00,
            WriteOpKind::Delete => 0x5EED,
            WriteOpKind::Update => 0x0BAD_F00D,
            WriteOpKind::ReplaceRowid => 0xC0FF_EE01,
            WriteOpKind::ReplaceUnique => 0xC0FF_EE02,
            WriteOpKind::ReplaceTwo => 0xC0FF_EE03,
        }
    }
}

/// One row of a write-amplification trace (spec §7's op table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    Insert {
        id: i64,
        email: String,
        handle: String,
        region: String,
        amount: i64,
    },
    Delete {
        id: i64,
    },
    Update {
        id: i64,
        region: String,
        amount: i64,
    },
    /// INSERT OR REPLACE of a full row; `expect_removed` rows are removed by
    /// conflict (spec §7's table: 1 for `replace_rowid` and
    /// `replace_unique`, 2 for `replace_two`).
    Replace {
        id: i64,
        email: String,
        handle: String,
        region: String,
        amount: i64,
        expect_removed: usize,
    },
}

/// Draw `count` distinct values from `0..upper` without replacement
/// (rejection sampling — the same technique `Workload::update_trace` already
/// uses for "an id that has not been deleted yet"). Every write-amp trace
/// needs this: no two ops in one trace may touch the same existing row
/// (spec §7).
///
/// # Panics
/// If `count > upper` (as `usize`): rejection sampling would never
/// terminate.
fn sample_distinct(rng: &mut StdRng, count: usize, upper: i64) -> Vec<i64> {
    assert!(
        upper >= 0 && count <= upper as usize,
        "cannot draw {count} distinct values without replacement from 0..{upper}"
    );
    let mut taken = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let id = rng.random_range(0..upper);
        if taken.insert(id) {
            out.push(id);
        }
    }
    out
}

/// The write-amplification workload (spec §7): its own schema (`accounts`),
/// data parameters, view counts, op kinds, transaction size and
/// `recursive_triggers` modes. Loaded from `workloads/write-amp.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteAmpWorkload {
    pub name: String,
    pub seed: u64,
    pub schema: WorkloadSchema,
    pub base_rows: usize,
    pub group_cardinality: usize,
    pub amount_max: i64,
    pub view_counts: Vec<usize>,
    pub ops: Vec<WriteOpKind>,
    pub rows_per_op: usize,
    pub tx_rows: usize,
    pub recursive_triggers: Vec<bool>,
    pub view_threshold_stride: i64,
    pub view_threshold_modulus: i64,
}

impl WriteAmpWorkload {
    pub fn load(path: &Path) -> Result<Self, WorkloadError> {
        let text = fs::read_to_string(path).map_err(WorkloadError::Io)?;
        let w: WriteAmpWorkload =
            toml::from_str(&text).map_err(|e| WorkloadError::Parse(e.to_string()))?;
        w.validate()?;
        Ok(w)
    }

    /// Reject every configuration `rows()`/`trace()`/`views()` cannot make
    /// sense of (M1b Phase 4 Task 5 fix round, ruling 9). `load` calls this,
    /// and so must anything that mutates a loaded `WriteAmpWorkload` before
    /// running it (`ivmlite-bench`'s `--views` override re-validates after
    /// replacing `view_counts`, for exactly this reason).
    ///
    /// `rows()` and `trace()` no longer clamp `group_cardinality` or
    /// `amount_max` to a minimum of 1 themselves — this is now the one place
    /// that rule lives, exactly as `Workload::validate` is the one place
    /// `group_cardinality > base_rows` is rejected.
    pub fn validate(&self) -> Result<(), WorkloadError> {
        if self.rows_per_op == 0 {
            return Err(WorkloadError::Invalid("rows_per_op must not be 0".into()));
        }
        if self.tx_rows == 0 {
            return Err(WorkloadError::Invalid("tx_rows must not be 0".into()));
        }
        // `replace_two` draws 2 * rows_per_op distinct existing rows per
        // trace (a pair per op); every other kind draws at most rows_per_op.
        let needs_two_rows_per_op = self.ops.contains(&WriteOpKind::ReplaceTwo);
        let min_base_rows = if needs_two_rows_per_op {
            2 * self.rows_per_op
        } else {
            self.rows_per_op
        };
        if self.base_rows < min_base_rows {
            return Err(WorkloadError::Invalid(format!(
                "base_rows ({}) must be at least {min_base_rows} \
                 ({}rows_per_op ({}){}, given ops)",
                self.base_rows,
                if needs_two_rows_per_op { "2 * " } else { "" },
                self.rows_per_op,
                if needs_two_rows_per_op {
                    " for replace_two's pair per op"
                } else {
                    ""
                },
            )));
        }
        if self.group_cardinality < 1 || self.group_cardinality > self.base_rows {
            return Err(WorkloadError::Invalid(format!(
                "group_cardinality ({}) must be between 1 and base_rows ({})",
                self.group_cardinality, self.base_rows
            )));
        }
        if self.amount_max <= 0 {
            return Err(WorkloadError::Invalid(format!(
                "amount_max ({}) must be positive",
                self.amount_max
            )));
        }
        if self.view_threshold_modulus <= 0 {
            return Err(WorkloadError::Invalid(format!(
                "view_threshold_modulus ({}) must be positive",
                self.view_threshold_modulus
            )));
        }
        if self.ops.is_empty() {
            return Err(WorkloadError::Invalid("ops must not be empty".into()));
        }
        if self.recursive_triggers.is_empty() {
            return Err(WorkloadError::Invalid(
                "recursive_triggers must not be empty".into(),
            ));
        }
        if self.view_counts.is_empty() {
            return Err(WorkloadError::Invalid(
                "view_counts must not be empty".into(),
            ));
        }
        Ok(())
    }

    /// A copy with `base_rows` and `rows_per_op` overridden. A test-only
    /// helper (spec §7's brief, Task 5 Step 1): it lets a test shrink the
    /// shipped workload without hand-editing every field of a clone.
    pub fn with_sizes(&self, base_rows: usize, rows_per_op: usize) -> Self {
        let mut w = self.clone();
        w.base_rows = base_rows;
        w.rows_per_op = rows_per_op;
        w
    }

    /// Base rows: id `i`, email `"e{i}@x"`, handle `"h{i}"`, region
    /// `"r{i % card}"`, amount seeded (spec §7's data description).
    ///
    /// Precondition: `group_cardinality >= 1` and `amount_max >= 1`.
    /// `validate` (which `load` calls) enforces both; a caller that builds a
    /// `WriteAmpWorkload` directly must uphold them itself, or `id % card`
    /// panics (division by zero) and `random_range(0..amount_max)` panics
    /// (an empty range).
    pub fn rows(&self) -> impl Iterator<Item = (i64, String, String, String, i64)> + '_ {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let card = self.group_cardinality as i64;
        let amount_max = self.amount_max;
        (0..self.base_rows as i64).map(move |id| {
            (
                id,
                format!("e{id}@x"),
                format!("h{id}"),
                format!("r{}", id % card),
                rng.random_range(0..amount_max),
            )
        })
    }

    /// The deterministic trace for one op kind, against the base rows only
    /// (spec §7): each kind's trace starts from a freshly seeded table, and
    /// every op targets a distinct existing row, drawn with the seeded RNG
    /// without replacement — so every op hits a live row, and no two ops in
    /// one trace touch the same one.
    ///
    /// A fresh email or handle (one not tied to any existing row) is
    /// prefixed `n`/`m` rather than the base rows' own `e`/`h`, so it can
    /// never be mistaken for pointing at an existing row.
    ///
    /// Precondition: the same as `rows()`'s, plus enough distinct rows for
    /// `kind` to draw without replacement (`validate` enforces this too:
    /// `base_rows >= rows_per_op`, or `>= 2 * rows_per_op` when `ops`
    /// contains `ReplaceTwo`) — otherwise `sample_distinct` panics.
    pub fn trace(&self, kind: WriteOpKind) -> Vec<WriteOp> {
        let mut rng = StdRng::seed_from_u64(self.seed ^ kind.seed_salt());
        let base = self.base_rows as i64;
        let m = self.rows_per_op;
        let card = self.group_cardinality as i64;
        let amount_max = self.amount_max;

        match kind {
            WriteOpKind::Insert => (0..m)
                .map(|i| {
                    let id = base + i as i64;
                    WriteOp::Insert {
                        id,
                        email: format!("e{id}@x"),
                        handle: format!("h{id}"),
                        region: format!("r{}", rng.random_range(0..card)),
                        amount: rng.random_range(0..amount_max),
                    }
                })
                .collect(),
            WriteOpKind::Delete => sample_distinct(&mut rng, m, base)
                .into_iter()
                .map(|id| WriteOp::Delete { id })
                .collect(),
            WriteOpKind::Update => sample_distinct(&mut rng, m, base)
                .into_iter()
                .map(|id| WriteOp::Update {
                    id,
                    region: format!("r{}", rng.random_range(0..card)),
                    amount: rng.random_range(0..amount_max),
                })
                .collect(),
            WriteOpKind::ReplaceRowid => sample_distinct(&mut rng, m, base)
                .into_iter()
                .map(|a| WriteOp::Replace {
                    id: a,
                    email: format!("n{a}@x"),
                    handle: format!("m{a}"),
                    region: format!("r{}", rng.random_range(0..card)),
                    amount: rng.random_range(0..amount_max),
                    expect_removed: 1,
                })
                .collect(),
            WriteOpKind::ReplaceUnique => sample_distinct(&mut rng, m, base)
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    let id = base + i as i64;
                    WriteOp::Replace {
                        id,
                        email: format!("e{a}@x"),
                        handle: format!("m{id}"),
                        region: format!("r{}", rng.random_range(0..card)),
                        amount: rng.random_range(0..amount_max),
                        expect_removed: 1,
                    }
                })
                .collect(),
            WriteOpKind::ReplaceTwo => sample_distinct(&mut rng, 2 * m, base)
                .chunks(2)
                .enumerate()
                .map(|(i, pair)| {
                    let (a, b) = (pair[0], pair[1]);
                    let id = base + i as i64;
                    WriteOp::Replace {
                        id,
                        email: format!("e{a}@x"),
                        handle: format!("h{b}"),
                        region: format!("r{}", rng.random_range(0..card)),
                        amount: rng.random_range(0..amount_max),
                        expect_removed: 2,
                    }
                })
                .collect(),
        }
    }

    /// The m0-shaped set of `n` views over `accounts` (spec §7: "views of
    /// the m0 shape over accounts"), using the same threshold formula
    /// `MatrixSpec::views` uses for the main matrix.
    pub fn views(&self, n: usize) -> Vec<ViewSpec> {
        (0..n)
            .map(|i| ViewSpec {
                id: i,
                threshold: (i as i64 * self.view_threshold_stride) % self.view_threshold_modulus,
            })
            .collect()
    }
}

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

    // ---- M1b Phase 4 Task 5 (spec §7): the write-amplification workload ----

    fn tiny() -> WriteAmpWorkload {
        WriteAmpWorkload::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/write-amp.toml"),
        )
        .unwrap()
        .with_sizes(1_000, 50) // base_rows, rows_per_op: a test-only helper
    }

    #[test]
    fn traces_are_deterministic_and_hit_distinct_live_rows() {
        let w = tiny();
        for kind in [
            WriteOpKind::Delete,
            WriteOpKind::Update,
            WriteOpKind::ReplaceRowid,
            WriteOpKind::ReplaceUnique,
            WriteOpKind::ReplaceTwo,
        ] {
            let t = w.trace(kind);
            assert_eq!(t, w.trace(kind), "{kind:?} is not deterministic");
            assert_eq!(t.len(), 50);
            let mut touched = BTreeSet::new();
            for op in &t {
                let hit: Vec<i64> = match op {
                    WriteOp::Delete { id } | WriteOp::Update { id, .. } => vec![*id],
                    WriteOp::Replace {
                        id,
                        email,
                        handle,
                        expect_removed,
                        ..
                    } => {
                        let mut ids = Vec::new();
                        if (*id as usize) < 1_000 {
                            ids.push(*id);
                        }
                        if let Some(a) = email.strip_prefix('e').and_then(|s| s.strip_suffix("@x"))
                        {
                            ids.push(a.parse().unwrap());
                        }
                        if let Some(b) = handle.strip_prefix('h') {
                            ids.push(b.parse().unwrap());
                        }
                        ids.dedup();
                        assert_eq!(ids.len(), *expect_removed, "{op:?}");
                        ids
                    }
                    WriteOp::Insert { .. } => vec![],
                };
                for id in hit {
                    assert!(
                        id < 1_000 && touched.insert(id),
                        "{kind:?} reuses or misses row {id}"
                    );
                }
            }
        }
    }

    /// `insert` is not covered by the distinctness test above (it never
    /// touches an existing row), so it gets its own: fresh ids starting
    /// exactly at `base_rows`, all distinct, and deterministic like every
    /// other kind.
    #[test]
    fn insert_trace_uses_fresh_ids_starting_at_base_rows() {
        let w = tiny();
        let t = w.trace(WriteOpKind::Insert);
        assert_eq!(t, w.trace(WriteOpKind::Insert));
        assert_eq!(t.len(), 50);
        let ids: BTreeSet<i64> = t
            .iter()
            .map(|op| match op {
                WriteOp::Insert { id, .. } => *id,
                other => panic!("insert trace produced a non-insert op: {other:?}"),
            })
            .collect();
        assert_eq!(ids.len(), 50, "every inserted id must be distinct");
        assert_eq!(*ids.iter().min().unwrap(), 1_000);
        assert_eq!(*ids.iter().max().unwrap(), 1_049);
    }

    #[test]
    fn rows_use_the_documented_id_email_handle_region_format() {
        let w = tiny();
        let first: Vec<_> = w.rows().take(3).collect();
        assert_eq!(
            first,
            vec![
                (
                    0,
                    "e0@x".to_string(),
                    "h0".to_string(),
                    "r0".to_string(),
                    first[0].4
                ),
                (
                    1,
                    "e1@x".to_string(),
                    "h1".to_string(),
                    "r1".to_string(),
                    first[1].4
                ),
                (
                    2,
                    "e2@x".to_string(),
                    "h2".to_string(),
                    "r2".to_string(),
                    first[2].4
                ),
            ]
        );
        for (_, _, _, _, amount) in w.rows() {
            assert!((0..200).contains(&amount), "amount {amount} out of range");
        }
    }

    #[test]
    fn write_amp_views_use_the_threshold_formula() {
        let w = tiny();
        let views = w.views(3);
        assert_eq!(views.len(), 3);
        for (i, v) in views.iter().enumerate() {
            assert_eq!(v.id, i);
            assert_eq!(
                v.threshold,
                (i as i64 * w.view_threshold_stride) % w.view_threshold_modulus
            );
        }
        assert!(views[1].sql("accounts").contains("GROUP BY region"));
    }

    #[test]
    fn shipped_write_amp_workload_file_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/write-amp.toml");
        let w = WriteAmpWorkload::load(&path).expect("the shipped workload file must parse");
        assert_eq!(w.name, "write-amp");
        assert_eq!(w.base_rows, 100_000);
        assert_eq!(w.group_cardinality, 1_000);
        assert_eq!(w.amount_max, 200);
        assert_eq!(w.view_counts, vec![0, 1, 10, 50, 200]);
        assert_eq!(
            w.ops,
            vec![
                WriteOpKind::Insert,
                WriteOpKind::Delete,
                WriteOpKind::Update,
                WriteOpKind::ReplaceRowid,
                WriteOpKind::ReplaceUnique,
                WriteOpKind::ReplaceTwo,
            ]
        );
        assert_eq!(w.rows_per_op, 1_000);
        assert_eq!(w.tx_rows, 1_000);
        assert_eq!(w.recursive_triggers, vec![false, true]);
        assert!(
            w.schema.ddl.contains("email")
                && w.schema.ddl.contains("UNIQUE")
                && w.schema.ddl.contains("handle"),
            "the accounts schema must have two ordinary UNIQUE keys besides the rowid (spec §7)"
        );
    }

    /// `sample_distinct` must actually draw *distinct* values — a mutation
    /// that dropped the `taken.insert` check (letting duplicates through)
    /// would make this red, since it directly counts distinctness rather
    /// than trusting `trace`'s own claim.
    #[test]
    fn sample_distinct_draws_are_unique_and_in_range() {
        let mut rng = StdRng::seed_from_u64(99);
        let drawn = sample_distinct(&mut rng, 40, 50);
        assert_eq!(drawn.len(), 40);
        assert_eq!(drawn.iter().collect::<BTreeSet<_>>().len(), 40);
        assert!(drawn.iter().all(|&id| (0..50).contains(&id)));
    }

    // ---- M1b Phase 4 Task 5 fix round, ruling 9: WriteAmpWorkload::validate ----

    /// Every rejection rule `validate` must enforce, each checked by
    /// mutating one field of a workload that is otherwise valid and
    /// confirming `validate()` returns `Err`. Mirrors the style of
    /// `Workload`'s own `validate` tests (e.g.
    /// `load_rejects_group_cardinality_exceeding_base_rows`): a direct call
    /// to `validate()`, not a round trip through TOML, since `validate`
    /// must be callable on its own (see the load-integration test below for
    /// the "load actually calls it" half).
    #[test]
    fn validate_rejects_every_invalid_field() {
        let base = tiny();
        assert!(base.validate().is_ok(), "the fixture itself must be valid");

        let cases: Vec<(&str, WriteAmpWorkload)> = vec![
            ("rows_per_op", {
                let mut w = base.clone();
                w.rows_per_op = 0;
                w
            }),
            ("tx_rows", {
                let mut w = base.clone();
                w.tx_rows = 0;
                w
            }),
            ("base_rows", {
                // No replace_two: base_rows must be >= rows_per_op (50).
                let mut w = base.clone();
                w.ops = vec![WriteOpKind::Delete];
                w.base_rows = 49;
                w
            }),
            ("base_rows", {
                // replace_two present: base_rows must be >= 2 * rows_per_op (100).
                let mut w = base.clone();
                w.ops = vec![WriteOpKind::ReplaceTwo];
                w.base_rows = 99;
                w
            }),
            ("group_cardinality", {
                let mut w = base.clone();
                w.group_cardinality = 0;
                w
            }),
            ("group_cardinality", {
                let mut w = base.clone();
                w.group_cardinality = w.base_rows + 1;
                w
            }),
            ("amount_max", {
                let mut w = base.clone();
                w.amount_max = 0;
                w
            }),
            ("view_threshold_modulus", {
                let mut w = base.clone();
                w.view_threshold_modulus = 0;
                w
            }),
            ("ops", {
                let mut w = base.clone();
                w.ops = vec![];
                w
            }),
            ("recursive_triggers", {
                let mut w = base.clone();
                w.recursive_triggers = vec![];
                w
            }),
            ("view_counts", {
                let mut w = base.clone();
                w.view_counts = vec![];
                w
            }),
        ];

        for (field, w) in cases {
            assert!(
                w.validate().is_err(),
                "validate() should reject an invalid {field}"
            );
        }
    }

    /// The boundary `base_rows == 2 * rows_per_op` (with `replace_two`
    /// present) must be accepted — an off-by-one in the comparison would
    /// reject this legal boundary too.
    #[test]
    fn validate_accepts_the_replace_two_base_rows_boundary() {
        let mut w = tiny();
        w.ops = vec![WriteOpKind::ReplaceTwo];
        w.base_rows = 2 * w.rows_per_op;
        w.group_cardinality = 1; // irrelevant here; must not exceed base_rows
        assert!(w.validate().is_ok());
    }

    /// `load` must actually call `validate`, not merely offer it (ruling
    /// 9): a shipped-file-shaped TOML with `rows_per_op = 0` must be
    /// rejected at `load` time.
    #[test]
    fn load_rejects_an_invalid_write_amp_workload() {
        let mut w = tiny();
        w.rows_per_op = 0;
        let toml_text = toml::to_string(&w).unwrap();

        let dir = std::env::temp_dir().join("ivmlite-workload-write-amp-invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, toml_text).unwrap();

        let err = WriteAmpWorkload::load(&path)
            .expect_err("rows_per_op = 0 must be rejected at load time");
        assert!(err.to_string().contains("rows_per_op"), "{err}");
    }
}
