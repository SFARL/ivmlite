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
    /// `pub`（而非仅 `load` 内部私用）：这条规则只应该有一个家。此前
    /// `ivmlite-bench/src/main.rs` 的 `variant()` 通过 clone + 改字段构造
    /// workload，绕开了 `load`，逼着调用方在 `main.rs` 里手工维护一份同样
    /// 的 `card > rows` 判断——两份拼法迟早会分叉。`variant()` 现在直接调
    /// 这个方法。
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
        }
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
}
