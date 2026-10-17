//! Versioned M2a fixtures and ordered SQL row mutations, independent of an engine.
//! This deliberately does not change the M1b workload or its random stream.
use std::collections::BTreeMap;
use std::path::Path;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::{reject_duplicates, WorkloadError};

fn invalid(message: impl Into<String>) -> WorkloadError {
    WorkloadError::Invalid(message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Single,
    Join,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataDistribution {
    Uniform,
    Zipf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLocality {
    Uniform,
    Hot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    UniformUniform,
    ZipfUniform,
    UniformHot,
    ZipfHot,
}
impl Profile {
    pub fn distribution(self) -> DataDistribution {
        match self {
            Self::UniformUniform | Self::UniformHot => DataDistribution::Uniform,
            Self::ZipfUniform | Self::ZipfHot => DataDistribution::Zipf,
        }
    }
    pub fn locality(self) -> TargetLocality {
        match self {
            Self::UniformUniform | Self::ZipfUniform => TargetLocality::Uniform,
            Self::UniformHot | Self::ZipfHot => TargetLocality::Hot,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::UniformUniform => "uniform_uniform",
            Self::ZipfUniform => "zipf_uniform",
            Self::UniformHot => "uniform_hot",
            Self::ZipfHot => "zipf_hot",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Table {
    Events,
    Lhs,
    Rhs,
}
impl Table {
    pub fn name(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Lhs => "lhs",
            Self::Rhs => "rhs",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Delete,
    Insert,
    Amount,
    Group,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct M2aConfig {
    pub schema_version: u32,
    pub name: String,
    pub seeds: Vec<u64>,
    pub profiles: Vec<Profile>,
    pub batch_sizes: Vec<usize>,
    pub fanouts: Vec<usize>,
    pub single_rows: usize,
    pub single_groups: usize,
    pub join_rows: usize,
    pub join_groups: usize,
    pub zipf_exponent: f64,
    pub hot_group_fraction: f64,
    pub hot_target_fraction: f64,
    pub amount_min: i64,
    pub amount_max: i64,
    pub warmup_rounds: usize,
    pub measured_rounds: usize,
    pub operation_order: Vec<OperationKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub shape: Shape,
    pub profile: Profile,
    pub base_rows: usize,
    pub group_cardinality: usize,
    /// Single-table cells use 1; join cells have this many rows per join key on each side.
    pub fanout: usize,
    pub batch_size: usize,
}
impl Cell {
    pub fn id(&self) -> String {
        let shape = match self.shape {
            Shape::Single => "single",
            Shape::Join => "join",
        };
        format!(
            "{shape}_{}_n{}_g{}_f{}_b{}",
            self.profile.label(),
            self.base_rows,
            self.group_cardinality,
            self.fanout,
            self.batch_size
        )
    }
    pub fn view_sql(&self) -> &'static str {
        match self.shape {
            Shape::Single => "SELECT g, COUNT(*), SUM(amount) FROM events GROUP BY g",
            Shape::Join => {
                "SELECT lhs.g, COUNT(*), SUM(rhs.amount) FROM lhs JOIN rhs ON lhs.k = rhs.k GROUP BY lhs.g"
            }
        }
    }
    pub fn schema_ddl(&self) -> &'static str {
        match self.shape {
            Shape::Single => {
                "CREATE TABLE events(id INTEGER PRIMARY KEY, g INTEGER NOT NULL, amount INTEGER NOT NULL) STRICT;"
            }
            Shape::Join => {
                "CREATE TABLE lhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL, g INTEGER NOT NULL) STRICT; CREATE TABLE rhs(id INTEGER PRIMARY KEY, k INTEGER NOT NULL, amount INTEGER NOT NULL) STRICT;"
            }
        }
    }
    pub fn ordinary_indexes(&self) -> &'static str {
        match self.shape {
            Shape::Single => "",
            Shape::Join => "CREATE INDEX lhs_k ON lhs(k); CREATE INDEX rhs_k ON rhs(k);",
        }
    }
    pub fn covering_indexes(&self) -> &'static str {
        match self.shape {
            Shape::Single => "CREATE INDEX events_g_amount ON events(g, amount);",
            Shape::Join => {
                "CREATE INDEX lhs_g_k ON lhs(g, k); CREATE INDEX rhs_k_amount ON rhs(k, amount);"
            }
        }
    }
}

/// Optional fields represent the three different fixed table schemas, never SQL NULL
/// in a performance fixture. Dedicated correctness fixtures can use SQL NULL separately.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: i64,
    pub k: Option<i64>,
    pub g: Option<i64>,
    pub amount: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub table: Table,
    pub before: Option<Row>,
    pub after: Option<Row>,
    /// Live group selected by the group-uniform or hot/cold target policy.
    /// For rhs UPDATE this is an lhs anchor, not all output groups affected.
    pub anchor_group: Option<i64>,
}
impl Mutation {
    pub fn inverse(&self) -> Self {
        Self {
            table: self.table,
            before: self.after.clone(),
            after: self.before.clone(),
            anchor_group: self.anchor_group,
        }
    }
    pub fn kind(&self) -> Result<OperationKind, WorkloadError> {
        match (&self.before, &self.after) {
            (None, Some(_)) => Ok(OperationKind::Insert),
            (Some(_), None) => Ok(OperationKind::Delete),
            (Some(a), Some(b))
                if a.id == b.id && a.k == b.k && a.g == b.g && a.amount != b.amount =>
            {
                Ok(OperationKind::Amount)
            }
            (Some(a), Some(b))
                if a.id == b.id && a.k == b.k && a.amount == b.amount && a.g != b.g =>
            {
                Ok(OperationKind::Group)
            }
            _ => Err(invalid(
                "mutation must be one INSERT, DELETE, non-noop amount UPDATE or group UPDATE",
            )),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub config: M2aConfig,
    pub cell: Cell,
    pub seed: u64,
    pub rounds: usize,
    pub tables: BTreeMap<Table, Vec<Row>>,
    /// One transaction followed by one refresh checkpoint per batch, in order.
    pub batches: Vec<Vec<Mutation>>,
}

impl M2aConfig {
    pub fn load(path: &Path) -> Result<Self, WorkloadError> {
        let text = std::fs::read_to_string(path).map_err(WorkloadError::Io)?;
        let config: Self =
            toml::from_str(&text).map_err(|e| WorkloadError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), WorkloadError> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported M2a schema_version"));
        }
        if self.name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        for (name, empty) in [
            ("seeds", self.seeds.is_empty()),
            ("profiles", self.profiles.is_empty()),
            ("batch_sizes", self.batch_sizes.is_empty()),
            ("fanouts", self.fanouts.is_empty()),
        ] {
            if empty {
                return Err(invalid(format!("{name} must not be empty")));
            }
        }
        reject_duplicates("M2a seeds", &self.seeds)?;
        reject_duplicates("M2a profiles", &self.profiles)?;
        reject_duplicates("M2a batch_sizes", &self.batch_sizes)?;
        reject_duplicates("M2a fanouts", &self.fanouts)?;
        for (name, rows, groups) in [
            ("single", self.single_rows, self.single_groups),
            ("join", self.join_rows, self.join_groups),
        ] {
            if groups < 2 || groups > rows {
                return Err(invalid(format!("{name} groups must be in 2..=rows")));
            }
            if i64::try_from(rows).is_err() {
                return Err(invalid(format!("{name} row id overflow")));
            }
            let hot = (groups as f64 * self.hot_group_fraction).floor() as usize;
            if hot == 0 || hot >= groups {
                return Err(invalid(format!(
                    "{name} hot partition must contain at least one and fewer than all groups"
                )));
            }
        }
        if !self.zipf_exponent.is_finite() || self.zipf_exponent <= 0.0 {
            return Err(invalid("zipf_exponent must be finite and positive"));
        }
        // Every rank must retain positive floating-point support in the
        // cumulative sampler, including the conditional non-noop draw.
        let ranks = self.single_groups.max(self.join_groups) as f64;
        if ranks.powf(-self.zipf_exponent) <= f64::EPSILON * ranks {
            return Err(invalid(
                "zipf_exponent and group count lose floating-point rank support",
            ));
        }
        for (name, value) in [
            ("hot_group_fraction", self.hot_group_fraction),
            ("hot_target_fraction", self.hot_target_fraction),
        ] {
            if !value.is_finite() || !(0.0..1.0).contains(&value) || value == 0.0 {
                return Err(invalid(format!(
                    "{name} must be finite and strictly between zero and one"
                )));
            }
        }
        if self.amount_min >= self.amount_max
            || self.amount_max.checked_sub(self.amount_min).is_none()
        {
            return Err(invalid(
                "amount range must contain at least two values without integer overflow",
            ));
        }
        if self.warmup_rounds == 0 || self.measured_rounds == 0 {
            return Err(invalid(
                "warmup_rounds and measured_rounds must be positive",
            ));
        }
        let rounds = self
            .warmup_rounds
            .checked_add(self.measured_rounds)
            .ok_or_else(|| invalid("round count overflow"))?;
        if self.operation_order.len() != 10 {
            return Err(invalid("operation_order must contain ten operations"));
        }
        for (kind, required) in [
            (OperationKind::Insert, 2),
            (OperationKind::Delete, 2),
            (OperationKind::Amount, 3),
            (OperationKind::Group, 3),
        ] {
            if self
                .operation_order
                .iter()
                .filter(|&&op| op == kind)
                .count()
                != required
            {
                return Err(invalid(
                    "operation_order must contain 2 INSERT, 2 DELETE, 3 amount and 3 group UPDATE operations",
                ));
            }
        }
        let mut pending = false;
        for kind in &self.operation_order {
            match kind {
                OperationKind::Delete if !pending => pending = true,
                OperationKind::Insert if pending => pending = false,
                OperationKind::Delete | OperationKind::Insert => {
                    return Err(invalid(
                        "INSERT must consume one preceding DELETE replacement",
                    ));
                }
                _ => (),
            }
        }
        for &batch in &self.batch_sizes {
            if batch == 0 || batch % self.operation_order.len() != 0 {
                return Err(invalid("batch_sizes must be positive multiples of ten"));
            }
            self.check_trace_size(batch, rounds)?;
        }
        for &fanout in &self.fanouts {
            if fanout == 0 || !self.join_rows.is_multiple_of(fanout) {
                return Err(invalid("fanout must be positive and divide join_rows"));
            }
            let joined = self
                .join_rows
                .checked_mul(fanout)
                .and_then(|n| i64::try_from(n).ok())
                .ok_or_else(|| invalid("joined row count overflow"))?;
            let largest = i128::from(self.amount_min)
                .abs()
                .max(i128::from(self.amount_max).abs());
            if i128::from(joined) * largest > i128::from(i64::MAX) {
                return Err(invalid("possible SUM(amount) overflow"));
            }
        }
        let largest = i128::from(self.amount_min)
            .abs()
            .max(i128::from(self.amount_max).abs());
        if self.single_rows as i128 * largest > i128::from(i64::MAX) {
            return Err(invalid("possible single SUM(amount) overflow"));
        }
        Ok(())
    }
    fn check_trace_size(&self, batch: usize, rounds: usize) -> Result<(), WorkloadError> {
        let count = batch
            .checked_mul(rounds)
            .ok_or_else(|| invalid("trace operation count overflow"))?;
        let inserted = count
            .checked_div(5)
            .ok_or_else(|| invalid("trace insertion count overflow"))?;
        self.single_rows
            .max(self.join_rows)
            .checked_add(inserted)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or_else(|| invalid("inserted primary key overflow"))?;
        Ok(())
    }
    pub fn cells(&self) -> Result<Vec<Cell>, WorkloadError> {
        self.validate()?;
        let mut cells = Vec::new();
        for &profile in &self.profiles {
            for &batch_size in &self.batch_sizes {
                cells.push(Cell {
                    shape: Shape::Single,
                    profile,
                    base_rows: self.single_rows,
                    group_cardinality: self.single_groups,
                    fanout: 1,
                    batch_size,
                });
                for &fanout in &self.fanouts {
                    cells.push(Cell {
                        shape: Shape::Join,
                        profile,
                        base_rows: self.join_rows,
                        group_cardinality: self.join_groups,
                        fanout,
                        batch_size,
                    });
                }
            }
        }
        reject_duplicates("M2a cells", &cells)?;
        Ok(cells)
    }
    pub fn total_rounds(&self) -> Result<usize, WorkloadError> {
        self.warmup_rounds
            .checked_add(self.measured_rounds)
            .ok_or_else(|| invalid("round count overflow"))
    }
    pub fn generate(&self, cell: &Cell, seed: u64) -> Result<Fixture, WorkloadError> {
        let rounds = self.total_rounds()?;
        self.generate_rounds(cell, seed, rounds)
    }
    pub fn generate_rounds(
        &self,
        cell: &Cell,
        seed: u64,
        rounds: usize,
    ) -> Result<Fixture, WorkloadError> {
        if !self.cells()?.contains(cell) {
            return Err(invalid("cell does not belong to the validated workload"));
        }
        if !self.seeds.contains(&seed) {
            return Err(invalid("seed does not belong to the workload"));
        }
        if rounds == 0 {
            return Err(invalid("rounds must be positive"));
        }
        self.check_trace_size(cell.batch_size, rounds)?;
        let mut rng = StdRng::seed_from_u64(seed);
        let sampler = GroupSampler::new(cell, self);
        let mut groups: Vec<i64> = (0..cell.group_cardinality as i64).collect();
        while groups.len() < cell.base_rows {
            groups.push(sampler.sample(&mut rng));
        }
        groups.shuffle(&mut rng);
        let mut tables = BTreeMap::new();
        let primary = match cell.shape {
            Shape::Single => Table::Events,
            Shape::Join => Table::Lhs,
        };
        tables.insert(
            primary,
            groups
                .into_iter()
                .enumerate()
                .map(|(id, g)| Row {
                    id: id as i64,
                    k: (cell.shape == Shape::Join).then_some((id / cell.fanout) as i64),
                    g: Some(g),
                    amount: (cell.shape == Shape::Single)
                        .then(|| rng.random_range(self.amount_min..=self.amount_max)),
                })
                .collect(),
        );
        if cell.shape == Shape::Join {
            tables.insert(
                Table::Rhs,
                (0..cell.base_rows)
                    .map(|id| Row {
                        id: id as i64,
                        k: Some((id / cell.fanout) as i64),
                        g: None,
                        amount: Some(rng.random_range(self.amount_min..=self.amount_max)),
                    })
                    .collect(),
            );
        }
        let mut live = Live::new(cell, &tables)?;
        let mut batches = Vec::with_capacity(rounds);
        let mut next_id = cell.base_rows as i64;
        for _ in 0..rounds {
            let mut batch = Vec::with_capacity(cell.batch_size);
            let mut replacement = None;
            for &kind in self.operation_order.iter().cycle().take(cell.batch_size) {
                let mutation = match kind {
                    OperationKind::Delete => {
                        let before = live.target(cell, self, &mut rng)?;
                        replacement = Some(before.k);
                        Mutation {
                            table: primary,
                            anchor_group: before.g,
                            before: Some(before),
                            after: None,
                        }
                    }
                    OperationKind::Insert => {
                        let k = replacement
                            .take()
                            .ok_or_else(|| invalid("INSERT has no replacement DELETE"))?;
                        let after = Row {
                            id: next_id,
                            k,
                            g: Some(sampler.sample(&mut rng)),
                            amount: (cell.shape == Shape::Single)
                                .then(|| rng.random_range(self.amount_min..=self.amount_max)),
                        };
                        next_id = next_id
                            .checked_add(1)
                            .ok_or_else(|| invalid("insert primary key overflow"))?;
                        Mutation {
                            table: primary,
                            before: None,
                            after: Some(after),
                            anchor_group: None,
                        }
                    }
                    OperationKind::Amount => {
                        let anchor = live.target(cell, self, &mut rng)?;
                        let before = if cell.shape == Shape::Single {
                            anchor.clone()
                        } else {
                            // rhs multiplicity is immutable and every join key exists.
                            let first = anchor.k.ok_or_else(|| invalid("lhs anchor missing k"))?
                                as usize
                                * cell.fanout;
                            live.rows[&Table::Rhs]
                                [&((first + rng.random_range(0..cell.fanout)) as i64)]
                                .clone()
                        };
                        let mut after = before.clone();
                        let mut amount = rng.random_range(self.amount_min..self.amount_max);
                        if amount
                            >= before
                                .amount
                                .ok_or_else(|| invalid("amount target missing amount"))?
                        {
                            amount += 1;
                        }
                        after.amount = Some(amount);
                        Mutation {
                            table: if cell.shape == Shape::Single {
                                Table::Events
                            } else {
                                Table::Rhs
                            },
                            before: Some(before),
                            after: Some(after),
                            anchor_group: anchor.g,
                        }
                    }
                    OperationKind::Group => {
                        let before = live.target(cell, self, &mut rng)?;
                        let mut after = before.clone();
                        // Exact conditional distribution, bounded even for extreme Zipf parameters.
                        after.g = Some(sampler.sample_other(&mut rng, before.g.unwrap())?);
                        Mutation {
                            table: primary,
                            anchor_group: before.g,
                            before: Some(before),
                            after: Some(after),
                        }
                    }
                };
                live.apply(&mutation, cell, self)?;
                batch.push(mutation);
            }
            live.checkpoint(cell)?;
            batches.push(batch);
        }
        let fixture = Fixture {
            schema_version: 1,
            config: self.clone(),
            cell: cell.clone(),
            seed,
            rounds,
            tables,
            batches,
        };
        fixture.validate()?;
        Ok(fixture)
    }
}

/// A cumulative Zipf sampler, with a separately normalized conditional sampler
/// for UPDATE destinations. Extreme exponents that lose rank support are rejected.
struct GroupSampler {
    cumulative: Vec<f64>,
}
impl GroupSampler {
    fn new(cell: &Cell, config: &M2aConfig) -> Self {
        let mut sum = 0.0;
        let cumulative = (1..=cell.group_cardinality)
            .map(|rank| {
                sum += match cell.profile.distribution() {
                    DataDistribution::Uniform => 1.0,
                    DataDistribution::Zipf => (rank as f64).powf(-config.zipf_exponent),
                };
                sum
            })
            .collect();
        Self { cumulative }
    }
    fn sample(&self, rng: &mut StdRng) -> i64 {
        let target = rng.random_range(0.0..*self.cumulative.last().unwrap());
        self.cumulative.partition_point(|&x| x <= target) as i64
    }
    fn sample_other(&self, rng: &mut StdRng, excluded: i64) -> Result<i64, WorkloadError> {
        let before = if excluded == 0 {
            0.0
        } else {
            self.cumulative[excluded as usize - 1]
        };
        let weight = self.cumulative[excluded as usize] - before;
        let remaining = self.cumulative.last().unwrap() - weight;
        if !remaining.is_finite() || remaining <= 0.0 {
            return Err(invalid(
                "Zipf exponent leaves no representable alternative group",
            ));
        }
        let mut target = rng.random_range(0.0..remaining);
        if target >= before {
            target += weight;
        }
        let selected = self.cumulative.partition_point(|&x| x <= target);
        if selected >= self.cumulative.len() || selected as i64 == excluded {
            return Err(invalid("Zipf conditional draw lost floating-point support"));
        }
        Ok(selected as i64)
    }
}

/// Indexed live eligibility avoids an O(base rows) scan on every hot target.
struct Live {
    rows: BTreeMap<Table, BTreeMap<i64, Row>>,
    groups: Vec<Vec<i64>>,
    positions: BTreeMap<i64, usize>,
    primary: Table,
}
impl Live {
    fn new(cell: &Cell, tables: &BTreeMap<Table, Vec<Row>>) -> Result<Self, WorkloadError> {
        let primary = if cell.shape == Shape::Single {
            Table::Events
        } else {
            Table::Lhs
        };
        let mut live = Self {
            rows: BTreeMap::new(),
            groups: vec![Vec::new(); cell.group_cardinality],
            positions: BTreeMap::new(),
            primary,
        };
        for (&table, rows) in tables {
            let mut map = BTreeMap::new();
            for row in rows {
                if map.insert(row.id, row.clone()).is_some() {
                    return Err(invalid("duplicate initial primary key"));
                }
                if table == primary {
                    live.add_group(row)?;
                }
            }
            live.rows.insert(table, map);
        }
        Ok(live)
    }
    fn add_group(&mut self, row: &Row) -> Result<(), WorkloadError> {
        let group = row
            .g
            .and_then(|g| usize::try_from(g).ok())
            .filter(|&g| g < self.groups.len())
            .ok_or_else(|| invalid("row group outside configured ranks"))?;
        self.positions.insert(row.id, self.groups[group].len());
        self.groups[group].push(row.id);
        Ok(())
    }
    fn remove_group(&mut self, row: &Row) -> Result<(), WorkloadError> {
        let group = row.g.ok_or_else(|| invalid("target missing group"))? as usize;
        let position = self
            .positions
            .remove(&row.id)
            .ok_or_else(|| invalid("target missing live eligibility"))?;
        self.groups[group].swap_remove(position);
        if let Some(&moved) = self.groups[group].get(position) {
            self.positions.insert(moved, position);
        }
        Ok(())
    }
    fn target(
        &self,
        cell: &Cell,
        config: &M2aConfig,
        rng: &mut StdRng,
    ) -> Result<Row, WorkloadError> {
        let hot = (cell.group_cardinality as f64 * config.hot_group_fraction).floor() as usize;
        let range = if cell.profile.locality() == TargetLocality::Uniform {
            0..self.groups.len()
        } else if rng.random_bool(config.hot_target_fraction) {
            0..hot
        } else {
            hot..self.groups.len()
        };
        let eligible: Vec<usize> = range.filter(|&g| !self.groups[g].is_empty()).collect();
        if eligible.is_empty() {
            return Err(invalid(
                "requested target partition has no live eligible group; no locality fallback permitted",
            ));
        }
        let group = eligible[rng.random_range(0..eligible.len())];
        let id = self.groups[group][rng.random_range(0..self.groups[group].len())];
        Ok(self.rows[&self.primary][&id].clone())
    }
    fn apply(
        &mut self,
        mutation: &Mutation,
        cell: &Cell,
        config: &M2aConfig,
    ) -> Result<(), WorkloadError> {
        mutation.kind()?;
        let row = mutation
            .before
            .as_ref()
            .or(mutation.after.as_ref())
            .ok_or_else(|| invalid("empty mutation"))?;
        let map = self
            .rows
            .get(&mutation.table)
            .ok_or_else(|| invalid("trace names an absent table"))?;
        if map.get(&row.id) != mutation.before.as_ref() {
            return Err(invalid(format!(
                "trace before-image differs from live {} PK {}",
                mutation.table.name(),
                row.id
            )));
        }
        if let Some(after) = &mutation.after {
            validate_row(mutation.table, after, cell, config)?;
        }
        if mutation.table == self.primary {
            if let Some(before) = &mutation.before {
                self.remove_group(before)?;
            }
            if let Some(after) = &mutation.after {
                self.add_group(after)?;
            }
        }
        let map = self.rows.get_mut(&mutation.table).unwrap();
        if let Some(after) = &mutation.after {
            map.insert(after.id, after.clone());
        } else {
            map.remove(&row.id);
        }
        Ok(())
    }
    fn checkpoint(&self, cell: &Cell) -> Result<(), WorkloadError> {
        let expected: &[Table] = match cell.shape {
            Shape::Single => &[Table::Events],
            Shape::Join => &[Table::Lhs, Table::Rhs],
        };
        if self.rows.len() != expected.len() {
            return Err(invalid("unexpected fixture tables"));
        }
        for &table in expected {
            let rows = self
                .rows
                .get(&table)
                .ok_or_else(|| invalid("missing fixture table"))?;
            if rows.len() != cell.base_rows {
                return Err(invalid(
                    "checkpoint base cardinality differs from initial fixture",
                ));
            }
            if cell.shape == Shape::Join {
                let mut counts = vec![0; cell.base_rows / cell.fanout];
                for row in rows.values() {
                    let k = row
                        .k
                        .and_then(|k| usize::try_from(k).ok())
                        .filter(|&k| k < counts.len())
                        .ok_or_else(|| invalid("join key outside initial domain"))?;
                    counts[k] += 1;
                }
                if counts.iter().any(|&count| count != cell.fanout) {
                    return Err(invalid("checkpoint join multiplicity differs from fanout"));
                }
            }
        }
        Ok(())
    }
}
fn validate_row(
    table: Table,
    row: &Row,
    cell: &Cell,
    config: &M2aConfig,
) -> Result<(), WorkloadError> {
    if row.id < 0 {
        return Err(invalid("negative performance-fixture primary key"));
    }
    let schema_matches = match table {
        Table::Events => {
            cell.shape == Shape::Single
                && row.k.is_none()
                && row.g.is_some()
                && row.amount.is_some()
        }
        Table::Lhs => {
            cell.shape == Shape::Join && row.k.is_some() && row.g.is_some() && row.amount.is_none()
        }
        Table::Rhs => {
            cell.shape == Shape::Join && row.k.is_some() && row.g.is_none() && row.amount.is_some()
        }
    };
    if !schema_matches {
        return Err(invalid("row fields do not match fixed STRICT table schema"));
    }
    if row
        .g
        .is_some_and(|g| g < 0 || g as usize >= cell.group_cardinality)
    {
        return Err(invalid("group rank out of range"));
    }
    if row
        .k
        .is_some_and(|k| k < 0 || k as usize >= cell.base_rows / cell.fanout)
    {
        return Err(invalid("join key out of range"));
    }
    if row
        .amount
        .is_some_and(|amount| !(config.amount_min..=config.amount_max).contains(&amount))
    {
        return Err(invalid("amount out of range"));
    }
    Ok(())
}
impl Fixture {
    /// Preflight actual exported rows and operations, not a newly regenerated trace.
    pub fn validate(&self) -> Result<(), WorkloadError> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported fixture schema_version"));
        }
        if !self.config.cells()?.contains(&self.cell) || !self.config.seeds.contains(&self.seed) {
            return Err(invalid("fixture cell/seed absent from workload"));
        }
        if self.rounds == 0 || self.batches.len() != self.rounds {
            return Err(invalid("fixture rounds/batches mismatch"));
        }
        self.config
            .check_trace_size(self.cell.batch_size, self.rounds)?;
        for (&table, rows) in &self.tables {
            for row in rows {
                validate_row(table, row, &self.cell, &self.config)?;
            }
        }
        let mut live = Live::new(&self.cell, &self.tables)?;
        live.checkpoint(&self.cell)?;
        if live.groups.iter().any(Vec::is_empty) {
            return Err(invalid("initial fixture must cover every group rank"));
        }
        for (batch_index, batch) in self.batches.iter().enumerate() {
            if batch.len() != self.cell.batch_size {
                return Err(invalid("fixture batch length mismatch"));
            }
            let mut deleted_key = None;
            for (index, mutation) in batch.iter().enumerate() {
                let expected =
                    self.config.operation_order[index % self.config.operation_order.len()];
                if mutation.kind()? != expected {
                    return Err(invalid("trace operation order differs from workload"));
                }
                let primary = if self.cell.shape == Shape::Single {
                    Table::Events
                } else {
                    Table::Lhs
                };
                let table = if expected == OperationKind::Amount && self.cell.shape == Shape::Join {
                    Table::Rhs
                } else {
                    primary
                };
                if mutation.table != table {
                    return Err(invalid("trace operation targets wrong table"));
                }
                if expected == OperationKind::Insert {
                    if mutation.anchor_group.is_some()
                        || Some(mutation.after.as_ref().unwrap().k) != deleted_key.take()
                    {
                        return Err(invalid(
                            "replacement INSERT must retain deleted join key and have no target anchor",
                        ));
                    }
                } else {
                    let anchor = mutation
                        .anchor_group
                        .and_then(|g| usize::try_from(g).ok())
                        .filter(|&g| g < live.groups.len())
                        .ok_or_else(|| {
                            invalid("existing-row operation missing legal anchor group")
                        })?;
                    if live.groups[anchor].is_empty() {
                        return Err(invalid("target anchor is not a live eligible group"));
                    }
                    let before = mutation.before.as_ref().unwrap();
                    if table == Table::Rhs {
                        if !live.groups[anchor]
                            .iter()
                            .any(|id| live.rows[&primary][id].k == before.k)
                        {
                            return Err(invalid(
                                "rhs target is unreachable from the recorded lhs anchor group",
                            ));
                        }
                    } else if before.g != Some(anchor as i64) {
                        return Err(invalid(
                            "target before-image differs from recorded anchor group",
                        ));
                    }
                    if expected == OperationKind::Delete {
                        deleted_key = Some(before.k);
                    }
                }
                live.apply(mutation, &self.cell, &self.config)
                    .map_err(|e| invalid(format!("batch {batch_index} operation {index}: {e}")))?;
            }
            live.checkpoint(&self.cell)?;
        }
        Ok(())
    }
    pub fn inverse_batches(&self) -> Vec<Vec<Mutation>> {
        self.batches
            .iter()
            .rev()
            .map(|batch| batch.iter().rev().map(Mutation::inverse).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn shipped() -> M2aConfig {
        M2aConfig::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/m2a.toml"))
            .unwrap()
    }
    fn tiny() -> M2aConfig {
        let mut config = shipped();
        config.single_rows = 4_000;
        config.single_groups = 20;
        config.join_rows = 4_000;
        config.join_groups = 20;
        config.fanouts = vec![1, 8];
        config.batch_sizes = vec![10, 100];
        config
    }
    #[test]
    fn shipped_matrix_has_32_cells_and_three_seeds() {
        let config = shipped();
        assert_eq!(config.cells().unwrap().len(), 32);
        assert_eq!(config.seeds, vec![1, 42, 2026]);
        assert_eq!(
            (
                config.single_rows,
                config.single_groups,
                config.join_rows,
                config.join_groups
            ),
            (100_000, 1_000, 32_768, 1_024)
        );
    }
    #[test]
    fn deterministic_portable_actual_replay_and_inverse() {
        let config = tiny();
        for cell in config.cells().unwrap() {
            let fixture = config.generate(&cell, 42).unwrap();
            assert_eq!(fixture, config.generate(&cell, 42).unwrap());
            let encoded = toml::to_string(&fixture).unwrap();
            let decoded: Fixture = toml::from_str(&encoded).unwrap();
            assert_eq!(fixture, decoded);
            decoded.validate().unwrap();
            let mut live = Live::new(&cell, &fixture.tables).unwrap();
            for batch in &fixture.batches {
                for mutation in batch {
                    live.apply(mutation, &cell, &config).unwrap();
                }
                live.checkpoint(&cell).unwrap();
            }
            for batch in fixture.inverse_batches() {
                for mutation in batch {
                    live.apply(&mutation, &cell, &config).unwrap();
                }
                live.checkpoint(&cell).unwrap();
            }
            let restored: BTreeMap<_, Vec<_>> = live
                .rows
                .into_iter()
                .map(|(table, rows)| (table, rows.into_values().collect()))
                .collect();
            assert_eq!(restored, fixture.tables);
        }
    }
    #[test]
    fn hot_policy_revisits_live_targets_and_zipf_changes_initial_distribution() {
        let mut config = tiny();
        config.single_rows = 10_000;
        config.join_rows = 8_000;
        config.single_groups = 100;
        config.join_groups = 100;
        config.batch_sizes = vec![1_000];
        config.fanouts = vec![8];
        let cells = config.cells().unwrap();
        let single = |profile| {
            cells
                .iter()
                .find(|c| c.shape == Shape::Single && c.profile == profile)
                .unwrap()
        };
        let uniform = config
            .generate(single(Profile::UniformUniform), 42)
            .unwrap();
        let hot = config.generate(single(Profile::ZipfHot), 42).unwrap();
        let share = |fixture: &Fixture| {
            fixture.tables[&Table::Events]
                .iter()
                .filter(|row| row.g.unwrap() < 10)
                .count() as f64
                / 10_000.0
        };
        assert!(share(&uniform) < 0.14);
        assert!(share(&hot) > 0.60);
        let targets: Vec<_> = hot
            .batches
            .iter()
            .flatten()
            .filter_map(|m| m.anchor_group)
            .collect();
        let hot_share = targets.iter().filter(|&&g| g < 10).count() as f64 / targets.len() as f64;
        assert!((0.76..0.84).contains(&hot_share), "{hot_share}");
        let ids: Vec<_> = hot
            .batches
            .iter()
            .flatten()
            .filter_map(|m| m.before.as_ref().map(|r| r.id))
            .collect();
        assert!(ids.iter().collect::<BTreeSet<_>>().len() < ids.len());
    }
    #[test]
    fn preflight_rejects_tampering_and_invalid_parameters() {
        let config = tiny();
        let cell = config
            .cells()
            .unwrap()
            .into_iter()
            .find(|c| c.shape == Shape::Join)
            .unwrap();
        let fixture = config.generate(&cell, 1).unwrap();
        let mut bad = fixture.clone();
        bad.batches[0][0].before.as_mut().unwrap().g = Some(999);
        assert!(bad.validate().is_err());
        let mut bad = fixture.clone();
        bad.batches[0][1].after.as_mut().unwrap().k = Some(999);
        assert!(bad.validate().is_err());
        let mut bad = fixture.clone();
        bad.batches[0][4].after = bad.batches[0][4].before.clone();
        assert!(bad.validate().is_err());
        let mut bad = fixture;
        bad.batches[0][4].anchor_group = Some(999);
        assert!(bad.validate().is_err());
        let edits: Vec<fn(&mut M2aConfig)> = vec![
            |c| c.zipf_exponent = f64::NAN,
            |c| c.hot_group_fraction = 0.0,
            |c| c.hot_target_fraction = 1.0,
            |c| c.fanouts = vec![3],
            |c| c.batch_sizes = vec![11],
            |c| c.profiles.push(c.profiles[0]),
            |c| c.seeds.push(c.seeds[0]),
            |c| c.amount_min = i64::MIN,
            |c| c.join_rows = usize::MAX,
            |c| c.measured_rounds = usize::MAX,
            |c| c.amount_max = i64::MAX,
        ];
        for edit in edits {
            let mut bad = config.clone();
            edit(&mut bad);
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn join_fanout_is_fixed_in_initial_rows_and_every_checkpoint() {
        let mut config = tiny();
        config.join_rows = 4_096;
        config.fanouts = vec![1, 8, 64];
        config.batch_sizes = vec![10];
        for cell in config
            .cells()
            .unwrap()
            .into_iter()
            .filter(|c| c.shape == Shape::Join)
        {
            let fixture = config.generate(&cell, 42).unwrap();
            let mut rows: BTreeMap<Table, BTreeMap<i64, Row>> = fixture
                .tables
                .iter()
                .map(|(&table, rs)| (table, rs.iter().cloned().map(|r| (r.id, r)).collect()))
                .collect();
            for batch in std::iter::once(&Vec::new()).chain(fixture.batches.iter()) {
                for mutation in batch {
                    let table = rows.get_mut(&mutation.table).unwrap();
                    if let Some(after) = &mutation.after {
                        table.insert(after.id, after.clone());
                    } else {
                        table.remove(&mutation.before.as_ref().unwrap().id);
                    }
                }
                let mut counts: BTreeMap<Table, BTreeMap<i64, usize>> = BTreeMap::new();
                for (&table, rs) in &rows {
                    for row in rs.values() {
                        *counts
                            .entry(table)
                            .or_default()
                            .entry(row.k.unwrap())
                            .or_default() += 1;
                    }
                }
                for table in [Table::Lhs, Table::Rhs] {
                    assert_eq!(rows[&table].len(), cell.base_rows);
                    assert_eq!(counts[&table].len(), cell.base_rows / cell.fanout);
                    assert!(counts[&table].values().all(|&n| n == cell.fanout));
                }
                let joined: usize = counts[&Table::Lhs]
                    .iter()
                    .map(|(k, n)| n * counts[&Table::Rhs][k])
                    .sum();
                assert_eq!(joined, cell.base_rows * cell.fanout);
            }
        }
    }
    #[test]
    fn load_calls_validation_and_rejects_duplicate_cells() {
        let config = tiny();
        let mut bad = config.clone();
        bad.batch_sizes.push(bad.batch_sizes[0]);
        let path =
            std::env::temp_dir().join(format!("ivmlite-m2a-invalid-{}.toml", std::process::id()));
        std::fs::write(&path, toml::to_string(&bad).unwrap()).unwrap();
        let result = M2aConfig::load(&path);
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap_err().to_string().contains("batch_sizes"));
    }
    #[test]
    fn unavailable_hot_partition_is_an_error_without_fallback() {
        let config = tiny();
        let cell = config
            .cells()
            .unwrap()
            .into_iter()
            .find(|c| c.profile == Profile::UniformHot)
            .unwrap();
        let fixture = config.generate(&cell, 42).unwrap();
        let mut live = Live::new(&cell, &fixture.tables).unwrap();
        let hot = (cell.group_cardinality as f64 * config.hot_group_fraction).floor() as usize;
        for group in &mut live.groups[..hot] {
            group.clear();
        }
        let mut rng = StdRng::seed_from_u64(42);
        assert!((0..100).any(|_| live.target(&cell, &config, &mut rng).is_err()));
    }
}
