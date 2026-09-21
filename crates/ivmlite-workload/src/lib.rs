use std::fs;
use std::path::Path;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

/// M0 只有 Uniform。这个枚举现在就存在，是为了 M2 加 Zipf 时
/// 不必改动 workload 文件格式（spec §10.6）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Distribution {
    Uniform,
}

/// 同上：M2 会加 Hot（更新集中打热 group）。
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSpec {
    pub id: usize,
    pub threshold: i64,
}

/// M0 基准矩阵的派生规则（spec §10.3 第 7 条）。这是
/// `docs/bench/m0-baseline.csv` 的唯一权威来源：`Workload::cells()` 读取
/// 这一段，产出每个测量格子的具体 `Workload`，取代此前活在
/// `ivmlite-bench/src/main.rs` 里的手写常量与两次扫描循环。任何引擎的
/// runner 只要加载同一份 workload 文件、调用 `cells()`，就能重新推导出
/// 与本仓库完全一致的格子集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixSpec {
    /// 两次扫描共用的基表规模取值。
    pub base_rows: Vec<usize>,
    /// 两次扫描共用的批大小取值。
    pub batch_sizes: Vec<usize>,
    /// 扫描二（视图数 × 基表规模 × 批大小）里视图数的取值。
    pub view_counts: Vec<usize>,
    /// 扫描一（group 基数 × 基表规模 × 批大小）里 group 基数的取值。
    pub group_cardinalities: Vec<usize>,
    /// 扫描一固定的视图数。
    pub fixed_views: usize,
    /// 扫描二固定的 group 基数。
    pub fixed_cardinality: usize,
    /// 第 i 个视图的阈值 = `(i * view_threshold_stride) % view_threshold_modulus`。
    pub view_threshold_stride: i64,
    pub view_threshold_modulus: i64,
}

impl MatrixSpec {
    /// 派生第 `n` 个视图的集合：阈值公式是让各视图彼此不同的手段，
    /// 与 `ivmlite-bench` 此前 `variant()` 里写死的公式完全一致
    /// （见本文件顶部关于 [matrix] 的说明）。
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
    /// M0 基准矩阵的派生规则（spec §10.3 第 7 条）。只有作为矩阵起点的
    /// workload 文件（如 `workloads/m0-baseline.toml`）需要这一段；由
    /// `cells()` 产出的具体格子里这个字段是 `None`——它们已经是矩阵求值
    /// 后的终点，不再需要一份求值规则。`#[serde(default)]` 让没有
    /// `[matrix]` 段的 workload 文件（以及现有构造 `Workload` 字面量的
    /// 测试代码）继续可以不提这个字段。
    #[serde(default)]
    pub matrix: Option<MatrixSpec>,
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
            WorkloadError::Parse(e) => write!(f, "解析 workload 失败: {e}"),
            WorkloadError::Invalid(e) => write!(f, "workload 配置不合法: {e}"),
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

    /// `group_cardinality > base_rows` 没有意义：N 行的表不可能容纳超过 N 个
    /// 不同的分组键。宁可在加载时就拒绝，也不要悄悄少生成——否则 benchmark
    /// 会在一个它其实没用过的基数下报告结果。
    ///
    /// `pub`（而非仅 `load` 内部私用）：这条规则只应该有一个家。`cells()`
    /// 直接调这个方法来判断一个矩阵格子是否合法；此前
    /// `ivmlite-bench/src/main.rs` 的 `variant()` + 手写的 `if card > rows`
    /// 把同一条规则拆成两份维护，两份拼法迟早会分叉——现在两处都只剩这一个
    /// 家。
    pub fn validate(&self) -> Result<(), WorkloadError> {
        if self.data.group_cardinality > self.data.base_rows {
            return Err(WorkloadError::Invalid(format!(
                "group_cardinality ({}) 不能大于 base_rows ({})",
                self.data.group_cardinality, self.data.base_rows
            )));
        }
        Ok(())
    }

    /// 基表行。id 稠密且唯一；分组键的不同值数量**精确**等于 group_cardinality
    /// ——前 card 行逐一覆盖每个键，其余行随机落入已有的键。随机落点无法保证
    /// 覆盖全部键，而 benchmark 依赖这个数字是准的。
    ///
    /// 前提：`base_rows >= group_cardinality`。这由 `validate`（`load` 会调用）
    /// 强制保证——反过来（分组键比行还多）没有意义，一张 N 行的表容不下超过
    /// N 个不同的键。调用方直接构造 `Workload`（不经过 `load`）时需自行保证
    /// 这一前提，否则分组键数量会悄悄退化为 `base_rows`。
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

    /// 一批更新。DELETE 一律命中已存在且未被删过的 id，INSERT 一律用新 id，
    /// 因此 trace 本身永远合法，任何 runner 直接重放即可，不需要各自维护
    /// 一份"当前还活着哪些行"的模型。
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

    /// 导出成任何引擎都能加载的形式：schema.sql / views.sql / data.csv /
    /// updates.csv。这是"workload 可移植"这条约束的实际兑现（spec §10.3 第 7 条）。
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

    /// 把 `[matrix]` 段展开成 M0 基准矩阵测的每一个具体格子（spec §10.3
    /// 第 7 条）：`docs/bench/m0-baseline.csv` 的 204 行就是三条基线各跑
    /// 一遍这里返回的格子集合。这取代了此前活在
    /// `ivmlite-bench/src/main.rs` 里的 `variant()` 加两个手写扫描循环——
    /// 把"怎么从一份基准 workload 派生出被测矩阵"这条规则搬进
    /// `ivmlite-workload`，任何引擎的 runner 只要加载同一份 workload 文件
    /// 调这个方法，就能重新推导出与本仓库完全一致的格子集合，不需要各自
    /// 重新实现两次扫描的结构、视图阈值公式，或下面的跳过规则。
    ///
    /// 保留**完全一致**的两次扫描结构（不是四维全交叉，四维会是 144 格，
    /// 过大——spec §10.1）：
    /// - 扫描一：group 基数 × 基表规模 × 批大小，视图数固定在
    ///   `matrix.fixed_views`。
    /// - 扫描二：视图数 × 基表规模 × 批大小，group 基数固定在
    ///   `matrix.fixed_cardinality`；跳过等于 `fixed_views` 的视图数，
    ///   避免与扫描一在 `(fixed_views, fixed_cardinality)` 这个公共点上
    ///   重复测量。
    ///
    /// 跳过规则：`group_cardinality > base_rows` 的组合不被发出——N 行的表
    /// 容不下超过 N 个不同分组键，这与 `validate()` 拒绝同一组合是**同一条
    /// 规则**，唯一的家在 `validate()`；这里只是不生成不合法的格子，不是
    /// 重新判断合法性。
    ///
    /// # Panics
    /// 如果 `self.matrix` 是 `None`（这个 workload 不是矩阵的起点），或者
    /// 派生出的某个格子未能通过 `validate()`（说明 `[matrix]` 本身写得不
    /// 自洽）。
    pub fn cells(&self) -> Vec<Workload> {
        let m = self
            .matrix
            .as_ref()
            .expect("cells() 需要 workload 里有 [matrix] 段");

        let mut cells = Vec::new();

        // 扫描一：group 基数 × 基表规模 × 批大小，视图数固定。
        for &card in &m.group_cardinalities {
            for &rows in &m.base_rows {
                if card > rows {
                    continue;
                }
                for &batch in &m.batch_sizes {
                    cells.push(self.cell(rows, card, m.fixed_views, batch, m));
                }
            }
        }

        // 扫描二：视图数 × 基表规模 × 批大小，group 基数固定。
        for &views in &m.view_counts {
            if views == m.fixed_views {
                continue; // 与扫描一的公共点重复，不重复测量。
            }
            for &rows in &m.base_rows {
                if m.fixed_cardinality > rows {
                    continue;
                }
                for &batch in &m.batch_sizes {
                    cells.push(self.cell(rows, m.fixed_cardinality, views, batch, m));
                }
            }
        }

        cells
    }

    /// `cells()` 的单格构造：clone 自身，改 `base_rows` /
    /// `group_cardinality` / `batch_size` / `views` 四个维度，返回前显式
    /// 调 `validate()`——这四个字段是 clone + 改字段构造出来的，绕过了
    /// `load()` 里的那次 `validate()`，所以这里要重新调一次，让
    /// `validate()` 仍然是 `group_cardinality > base_rows` 这条规则唯一的
    /// 执行点。派生出的格子已经是矩阵求值后的具体配置，`matrix` 字段清成
    /// `None`——它不再需要一份求值规则。
    fn cell(
        &self,
        base_rows: usize,
        group_cardinality: usize,
        views: usize,
        batch_size: usize,
        m: &MatrixSpec,
    ) -> Workload {
        let mut w = self.clone();
        w.data.base_rows = base_rows;
        w.data.group_cardinality = group_cardinality;
        w.updates.batch_size = batch_size;
        w.views = m.views(views);
        w.matrix = None;
        w.validate()
            .unwrap_or_else(|e| panic!("cells() 构造出了非法 workload: {e}"));
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
            "不同分组键的数量必须精确等于 group_cardinality——这是 benchmark 的核心维度"
        );
    }

    /// 缺口测试（M1a Phase 1 Task 1 变异审计新增）：`rows_respect_group_cardinality`
    /// 用 500 行、7 个分组键跑纯随机分配也几乎必然覆盖全部 7 个键（`rows()`
    /// 若把"前 card 行逐一覆盖每个键"改成"每一行都纯随机落点"，把变异真的
    /// 跑一遍验证时，那条测试仍然是绿的——不是巧合失败，就是没抓到）。真正
    /// 抓住这次变异的是 `base_rows_equal_to_group_cardinality_is_accepted`，
    /// 但那条测试的名字与断言意图都是"边界值被接受"，不是"分组键覆盖精确"，
    /// 它能抓到纯属该场景样本量小（7 个 draw 覆盖 7 个键的概率很低）的副作用。
    ///
    /// 直接把文档注释里声称的机制（"前 card 行逐一覆盖每个键"）钉成断言：
    /// 不看最终不同值的数量，而看前 `card` 行的分组键是不是精确按
    /// `0..card` 顺序出现。纯随机分配几乎不可能巧合出这个顺序。
    #[test]
    fn first_card_rows_deterministically_cover_each_group_in_order() {
        let w = spec();
        let card = w.data.group_cardinality;
        let regions: Vec<String> = w.rows().take(card).map(|(_, r, _)| r).collect();
        let want: Vec<String> = (0..card).map(|i| format!("r{i}")).collect();
        assert_eq!(
            regions, want,
            "前 group_cardinality 行必须逐一、按序覆盖每个分组键（spec §10.1）——\
             这是覆盖率精确的保证机制本身，而不是让后续随机采样'大概率'凑齐"
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
                    assert!(live.insert(id), "trace 不得重复插入同一个 id");
                }
                TraceOp::Delete { id } => {
                    assert!(live.remove(&id), "trace 里的 DELETE 必须命中存在的 id");
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
        let w = Workload::load(&path).expect("发布的 workload 文件必须可解析");
        assert_eq!(w.name, "m0-baseline");
        assert!(!w.views.is_empty());
        assert!(
            w.schema.ddl.contains("INTEGER PRIMARY KEY"),
            "spec §10.3 第 5 条要求稳定主键"
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
            assert!(dir.join(f).exists(), "缺少导出产物 {f}");
        }
        let data = std::fs::read_to_string(dir.join("data.csv")).unwrap();
        assert_eq!(data.lines().count(), 51, "表头 + 50 行");
    }

    /// benchmark runner 通过 clone + mutate 一份 base workload 派生每个配置变体
    /// （见任务约束）；克隆出来的副本必须与原件独立，改一个不能动到另一个。
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

    /// 额外一条（ruling-review #2）：`validate` 必须能在不经过 `load`
    /// （即不落盘再解析 TOML）的情况下被直接调用——这是 `ivmlite-bench`
    /// 的 `variant()` 能够复用它,而不必自己再维护一份同样规则的前提。
    #[test]
    fn validate_is_directly_callable_without_going_through_load() {
        let mut w = spec();
        w.data.base_rows = 5;
        w.data.group_cardinality = 50;
        let err = w.validate().expect_err("card > base_rows 必须被拒绝");
        let msg = err.to_string();
        assert!(msg.contains("50") && msg.contains('5'));

        w.data.group_cardinality = 5;
        assert!(w.validate().is_ok(), "card == base_rows 是合法边界");
    }

    /// group_cardinality > base_rows 没有意义（N 行的表容不下超过 N 个分组键）；
    /// load 必须在加载时就拒绝，而不是悄悄生成更少的分组键。
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

        let err = Workload::load(&path).expect_err("group_cardinality > base_rows 必须被拒绝");
        let msg = err.to_string();
        assert!(msg.contains("100"), "{msg}");
        assert!(msg.contains("10"), "{msg}");
    }

    /// base_rows == group_cardinality 是合法边界（每行自成一组）；比较里的
    /// 差一错误会把这个边界也拒掉，所以要单独断言它被接受且分组键数量精确。
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

        let loaded = Workload::load(&path).expect("base_rows == group_cardinality 必须被接受");
        let regions: BTreeSet<String> = loaded.rows().map(|(_, r, _)| r).collect();
        assert_eq!(regions.len(), 7);
    }

    /// `cells()` 必须是确定性的——同样的输入两次调用产出同样的输出。
    /// 这既是"文件完全决定格子集合"（spec §10.3 第 7 条）的直接要求，也是
    /// CSV-一致性测试能够成立的前提：如果两次调用能给出不同结果，"cells()
    /// 产出的集合与 CSV 里的集合相等"这句话就没有意义。
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
            "cells() 两次调用必须给出完全相同的格子序列"
        );
    }

    /// `cells()` 派生出的每一格都必须是一个合法 workload：clone + 改字段
    /// 绕过了 `load()` 里的 `validate()`，`cell()` 内部要重新调一次——这条
    /// 测试直接断言这个不变式成立，而不是只信任实现里的注释。
    #[test]
    fn every_cell_passes_validate() {
        let w = spec_with_matrix();
        for c in w.cells() {
            c.validate()
                .unwrap_or_else(|e| panic!("cells() 产出了未通过 validate() 的格子: {e}"));
        }
    }

    /// 跳过规则的直接断言：没有任何一个发出的格子满足
    /// `group_cardinality > base_rows`。`matrix_spec()` 里的
    /// `group_cardinalities` 故意包含 5000——大于所有 `base_rows` 取值——
    /// 用来触发这条规则。
    #[test]
    fn no_emitted_cell_has_cardinality_exceeding_base_rows() {
        let w = spec_with_matrix();
        for c in w.cells() {
            assert!(
                c.data.group_cardinality <= c.data.base_rows,
                "cells() 不应该发出 group_cardinality={} > base_rows={} 的格子",
                c.data.group_cardinality,
                c.data.base_rows
            );
        }
    }

    /// 跳过规则确实在起作用：把两次扫描按"不做任何跳过"直接算出的朴素笛卡尔积
    /// 大小，与 `cells()` 实际产出的数量相比，后者必须更少。`matrix_spec()`
    /// 里 `group_cardinalities` 含 5000（恒大于所有 `base_rows`）、
    /// `fixed_cardinality=50` 对 `base_rows=10` 也不成立，两条路径都会触发
    /// 跳过。
    #[test]
    fn skip_rule_emits_fewer_cells_than_naive_cross_product() {
        let w = spec_with_matrix();
        let m = w.matrix.as_ref().unwrap();

        // 扫描一朴素笛卡尔积：不检查 card > rows。
        let naive_scan_one = m.group_cardinalities.len() * m.base_rows.len() * m.batch_sizes.len();
        // 扫描二朴素笛卡尔积：仍然排除与扫描一重复的 fixed_views 公共点
        // （这是两次扫描的结构本身，不是跳过规则），但不检查 card > rows。
        let naive_scan_two = (m.view_counts.len() - 1) * m.base_rows.len() * m.batch_sizes.len();
        let naive_total = naive_scan_one + naive_scan_two;

        let actual = w.cells().len();
        assert!(
            actual < naive_total,
            "跳过规则应当让 cells() 产出的数量（{actual}）少于朴素笛卡尔积（{naive_total}）"
        );
    }
}
