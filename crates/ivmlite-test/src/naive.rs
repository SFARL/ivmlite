use std::cell::RefCell;
use std::collections::BTreeMap;

use ivmlite_core::{
    parse_numeric_text, ColumnType, Database, NumericTerm, Row, Value, ZSet, SUM_OVERFLOW,
};
use rusqlite::Connection;

use crate::exact_reference::{exact_mean, fsum, int_pieces};
use crate::{AggFn, CmpOp, Column, Engine, EngineError, Predicate, ViewQuery};

/// The trivially correct reference implementation: it keeps the full base
/// tables and recomputes on every materialize, with `evaluate`.
///
/// Two uses: showing the test framework raises no false alarms, and serving as the benchmark's "naive recompute" baseline (spec §10.2).
///
/// `apply` and `refresh` are genuinely separate phases: `apply` only appends the
/// raw `(table, Row, i64)` to `pending`, merging nothing, and `refresh` drains
/// `pending` into `base` table by table. If `apply` merged eagerly, `refresh`
/// would be a no-op and no engine ignoring the refresh contract would be caught
/// (spec §8.2).
///
/// `base` is held per table (`BTreeMap<String, ZSet>`) — since the harness went
/// multi-table, the initial state `create_view` receives is split by table
/// anyway.
#[derive(Debug)]
pub struct NaiveRecompute {
    view: Option<(Database, ViewQuery)>,
    base: BTreeMap<String, ZSet>,
    pending: Vec<(String, Row, i64)>,
    terms: SqliteTerms,
}

impl Default for NaiveRecompute {
    fn default() -> Self {
        NaiveRecompute {
            view: None,
            base: BTreeMap::new(),
            pending: Vec::new(),
            terms: SqliteTerms::new().expect("an in-memory SQLite opens"),
        }
    }
}

impl NaiveRecompute {
    pub fn new() -> Self {
        Self::default()
    }
}

/// SQLite's own classification of a SUM/TOTAL/AVG input (M2c spec §4): the
/// reference asks SQLite rather than `ivmlite_core::numeric_term`, so the
/// engine's reading of a value is checked against SQLite's.
#[derive(Debug)]
pub struct SqliteTerms {
    conn: Connection,
    cache: RefCell<BTreeMap<Value, SqliteTerm>>,
}

/// What `SELECT sum(?1)` returns for one value: an INTEGER for a value that
/// counts as an integer, a REAL for anything else.
#[derive(Debug, Clone, Copy)]
enum SqliteTerm {
    Integer(i64),
    Approx(f64),
}

impl SqliteTerms {
    pub fn new() -> Result<SqliteTerms, EngineError> {
        Ok(SqliteTerms {
            conn: Connection::open_in_memory().map_err(|e| EngineError(e.to_string()))?,
            cache: RefCell::new(BTreeMap::new()),
        })
    }

    fn term(&self, value: &Value) -> Result<SqliteTerm, EngineError> {
        if let Some(term) = self.cache.borrow().get(value) {
            return Ok(*term);
        }
        let got: rusqlite::types::Value = self
            .conn
            .query_row("SELECT sum(?1)", [crate::oracle::to_sql(value)], |r| {
                r.get(0)
            })
            .map_err(|e| EngineError(e.to_string()))?;
        let term = match got {
            rusqlite::types::Value::Integer(n) => SqliteTerm::Integer(n),
            rusqlite::types::Value::Real(x) => SqliteTerm::Approx(x),
            other => {
                return Err(EngineError(format!(
                    "SQLite's sum({value:?}) is {other:?}, neither INTEGER nor REAL"
                )))
            }
        };
        self.cache.borrow_mut().insert(value.clone(), term);
        Ok(term)
    }
}

/// The view as M2c specifies it, recomputed from the base multisets.
#[derive(Debug, Clone)]
pub struct Evaluated {
    pub rows: ZSet,
    /// (Σ|xᵢ|, non-NULL count) of each SUM/TOTAL/AVG, by canonical group key and aggregate index.
    pub magnitudes: BTreeMap<(Row, usize), (f64, i64)>,
}

/// The view as M2c specifies it (M2c spec §6, the independent reference),
/// recomputed from `bases` with no incremental state: SQLite classifies each
/// SUM/TOTAL/AVG input, `fsum` and `exact_mean` round, and nothing comes
/// from `ivmlite_core::ExactSum`.
///
/// `Err(EngineError(SUM_OVERFLOW))` when a final exact SUM over integers
/// leaves `i64` (M2c spec §4).
pub fn evaluate(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
    terms: &SqliteTerms,
) -> Result<Evaluated, EngineError> {
    if let Some(agg) = query
        .aggs
        .iter()
        .find(|a| a.func != AggFn::Count && a.column.is_none())
    {
        return Err(EngineError(format!("{:?} has no column", agg.func)));
    }
    let columns = joined_columns(db, query)?;
    let mut groups: BTreeMap<Row, Group> = BTreeMap::new();
    for (row, &weight) in input_rows(db, query, bases)?.iter() {
        if !passes(&query.predicate, row, &columns) {
            continue;
        }
        let key = Row::new(
            query
                .group_by
                .iter()
                .map(|&i| row.get(i).canonical())
                .collect(),
        );
        groups
            .entry(key)
            .or_insert_with(|| Group::new(query))
            .add(query, row, weight, terms)?;
    }

    let mut rows = ZSet::new();
    let mut magnitudes = BTreeMap::new();
    for (key, group) in groups {
        let mut values: Vec<Value> = key
            .0
            .iter()
            .zip(&group.real_keys)
            .map(|(k, &reals)| output_key(k, reals))
            .collect();
        for (i, (agg, slot)) in query.aggs.iter().zip(&group.slots).enumerate() {
            values.push(slot.output(agg.func, agg.column.is_some(), group.rows)?);
            if matches!(agg.func, AggFn::Sum | AggFn::Total | AggFn::Avg) {
                magnitudes.insert((key.clone(), i), (slot.magnitude(), slot.non_null));
            }
        }
        rows.update(Row::new(values), 1);
    }
    Ok(Evaluated { rows, magnitudes })
}

/// M2c spec §2: a group's numeric key is output as REAL when any of its rows
/// stored that key as a REAL.
fn output_key(canonical: &Value, reals: i64) -> Value {
    match canonical {
        Value::Int(n) if reals > 0 => Value::Real(*n as f64),
        other => other.clone(),
    }
}

/// The columns of the rows `input_rows` produces: the anchor's, then — for a
/// join — the right table's.
fn joined_columns<'a>(db: &'a Database, query: &ViewQuery) -> Result<Vec<&'a Column>, EngineError> {
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database has no tables".into()))?;
    let mut columns: Vec<&Column> = anchor.columns.iter().collect();
    if let Some(join) = &query.join {
        let right = db.get(&join.right).ok_or_else(|| {
            EngineError(format!(
                "the query joins {}, which the database does not declare",
                join.right
            ))
        })?;
        columns.extend(&right.columns);
    }
    Ok(columns)
}

/// The rows the view aggregates: the anchor's base state (the first table in
/// `db`), or — for a join — every matching pair of anchor and right rows,
/// concatenated, with the product of their weights. Deliberately the
/// simplest correct algorithm, a nested loop sharing no code with
/// `JoinState`.
///
/// Join keys match when their canonical values are equal (M2c spec §5: `1`
/// matches `1.0`). A NULL key never matches: SQL's `NULL = NULL` is UNKNOWN
/// (measured in SQLite; `sqlite_never_matches_null_join_keys` pins it on the
/// oracle side).
///
/// Each side's rows with weight ≤ 0 are dropped **before** joining (spec
/// §5.1: they do not take part in recomputation). Dropping them only after
/// the join would let two negative rows join into a positive one.
fn input_rows(
    db: &Database,
    query: &ViewQuery,
    bases: &BTreeMap<String, ZSet>,
) -> Result<ZSet, EngineError> {
    let anchor = db
        .tables()
        .first()
        .ok_or_else(|| EngineError("database has no tables".into()))?;
    let base = |table: &str| positive_part(bases.get(table).unwrap_or(&ZSet::new()));
    let anchor = base(&anchor.table);
    let Some(join) = &query.join else {
        return Ok(anchor);
    };
    let right = base(&join.right);
    let mut joined = ZSet::new();
    for (l, &wl) in anchor.iter() {
        let key = l.get(join.left_column).canonical();
        if key == Value::Null {
            continue;
        }
        for (r, &wr) in right.iter() {
            if key == r.get(join.right_column).canonical() {
                let mut values = l.0.clone();
                values.extend(r.0.iter().cloned());
                joined.update(Row::new(values), wl * wr);
            }
        }
    }
    Ok(joined)
}

/// The rows of `z` with a positive weight.
fn positive_part(z: &ZSet) -> ZSet {
    ZSet::from_rows(
        z.iter()
            .filter(|(_, &w)| w > 0)
            .map(|(row, &w)| (row.clone(), w)),
    )
}

/// M2c spec §5: a comparison literal after the column's affinity is applied
/// to it (SQLite's datatype3.html §4.2). A TEXT literal that parses as a
/// number meets a numeric-affinity column as that number, in its comparison
/// form (`'3.0'` is `3`); an INTEGER literal meets a TEXT-affinity column as
/// its decimal text; everything else is unchanged. (No REAL literal meets a
/// TEXT-affinity column: the engine refuses that view.)
fn affinity_literal(ty: ColumnType, literal: &Value) -> Value {
    match (ty, literal) {
        (ColumnType::Integer | ColumnType::Real | ColumnType::Numeric, Value::Text(t)) => {
            match parse_numeric_text(t) {
                Some(NumericTerm::Integer(n)) => Value::Int(n),
                Some(NumericTerm::Approx(x)) => Value::Real(x).canonical(),
                None => literal.clone(),
            }
        }
        (ColumnType::Text, Value::Int(n)) => Value::Text(n.to_string()),
        _ => literal.clone(),
    }
}

/// Whether `row` passes `predicate`, with `columns` describing the row. A
/// comparison follows M2c spec §2's cross-class order.
fn passes(predicate: &Predicate, row: &Row, columns: &[&Column]) -> bool {
    match predicate {
        Predicate::None => true,
        Predicate::Compare { column, op, value } => {
            let cell = row.get(*column);
            // NULL <op> anything is UNKNOWN (spec §6.1), for `!=` too.
            if *cell == Value::Null {
                return false;
            }
            let order = cell.sql_cmp(&affinity_literal(columns[*column].ty, value));
            match op {
                CmpOp::Gt => order.is_gt(),
                CmpOp::Ge => order.is_ge(),
                CmpOp::Lt => order.is_lt(),
                CmpOp::Le => order.is_le(),
                CmpOp::Eq => order.is_eq(),
                CmpOp::Ne => order.is_ne(),
            }
        }
        Predicate::IsNull { column } => row.get(*column) == &Value::Null,
        Predicate::IsNotNull { column } => row.get(*column) != &Value::Null,
    }
}

/// One group's state while `evaluate` scans its rows.
struct Group {
    /// Rows, by weight: COUNT(*).
    rows: i64,
    /// Per group-by column, the rows whose key was a REAL, by weight.
    real_keys: Vec<i64>,
    slots: Vec<Slot>,
}

impl Group {
    fn new(query: &ViewQuery) -> Group {
        Group {
            rows: 0,
            real_keys: vec![0; query.group_by.len()],
            slots: query.aggs.iter().map(|_| Slot::default()).collect(),
        }
    }

    fn add(
        &mut self,
        query: &ViewQuery,
        row: &Row,
        weight: i64,
        terms: &SqliteTerms,
    ) -> Result<(), EngineError> {
        self.rows += weight;
        for (reals, &c) in self.real_keys.iter_mut().zip(&query.group_by) {
            if matches!(row.get(c), Value::Real(_)) {
                *reals += weight;
            }
        }
        for (slot, agg) in self.slots.iter_mut().zip(&query.aggs) {
            let Some(c) = agg.column else {
                continue; // COUNT(*) reads `rows`
            };
            let value = row.get(c);
            if *value == Value::Null {
                continue;
            }
            slot.non_null += weight;
            match agg.func {
                AggFn::Count => {}
                AggFn::Min | AggFn::Max => slot.values.push(value.clone()),
                AggFn::Sum | AggFn::Total | AggFn::Avg => slot.add_term(terms.term(value)?, weight),
            }
        }
        Ok(())
    }
}

/// One aggregate's inputs within a group.
#[derive(Default)]
struct Slot {
    non_null: i64,
    /// The exact sum of the terms SQLite counts as integers.
    int_sum: i128,
    /// Σ|n|·w of those integer terms, for KBN's bound.
    int_magnitude: i128,
    /// Each finite non-integer term, repeated by weight.
    approx: Vec<f64>,
    pos_inf: i64,
    neg_inf: i64,
    /// MIN/MAX's non-NULL values.
    values: Vec<Value>,
}

impl Slot {
    fn add_term(&mut self, term: SqliteTerm, weight: i64) {
        match term {
            SqliteTerm::Integer(n) => {
                self.int_sum += i128::from(n) * i128::from(weight);
                self.int_magnitude += i128::from(n).abs() * i128::from(weight);
            }
            SqliteTerm::Approx(x) if x == f64::INFINITY => self.pos_inf += weight,
            SqliteTerm::Approx(x) if x == f64::NEG_INFINITY => self.neg_inf += weight,
            SqliteTerm::Approx(x) => {
                let copies = usize::try_from(weight).expect("a positive weight");
                self.approx.extend(std::iter::repeat_n(x, copies));
            }
        }
    }

    /// M2c spec §4: a group is approximate once any input is not an integer.
    fn is_approx(&self) -> bool {
        !self.approx.is_empty() || self.pos_inf > 0 || self.neg_inf > 0
    }

    /// Finite terms whose exact sum is the sum of every finite input.
    fn terms(&self) -> Vec<f64> {
        let mut all = self.approx.clone();
        all.extend(int_pieces(self.int_sum));
        all
    }

    /// Σ|xᵢ| over every finite input, integers included (KBN's bound).
    fn magnitude(&self) -> f64 {
        let mut all: Vec<f64> = self.approx.iter().map(|x| x.abs()).collect();
        all.extend(int_pieces(self.int_magnitude));
        fsum(&all)
    }

    /// A REAL result: `finish` of the finite terms, unless an infinity is
    /// present (M2c spec §4, amendment 5: both signs give NULL, SQLite's NaN).
    fn real(&self, finish: impl Fn(&[f64]) -> f64) -> Value {
        match (self.pos_inf > 0, self.neg_inf > 0) {
            (true, true) => Value::Null,
            (true, false) => Value::Real(f64::INFINITY),
            (false, true) => Value::Real(f64::NEG_INFINITY),
            (false, false) => Value::Real(finish(&self.terms())),
        }
    }

    fn output(&self, func: AggFn, has_column: bool, rows: i64) -> Result<Value, EngineError> {
        Ok(match func {
            AggFn::Count if has_column => Value::Int(self.non_null),
            AggFn::Count => Value::Int(rows),
            AggFn::Sum | AggFn::Avg if self.non_null == 0 => Value::Null,
            AggFn::Sum if self.is_approx() => self.real(fsum),
            AggFn::Sum => i64::try_from(self.int_sum)
                .map(Value::Int)
                .map_err(|_| EngineError(SUM_OVERFLOW.into()))?,
            AggFn::Total if self.non_null == 0 => Value::Real(0.0),
            AggFn::Total => self.real(fsum),
            AggFn::Avg => self.real(|t| exact_mean(t, self.non_null)),
            AggFn::Min => extreme(&self.values, false),
            AggFn::Max => extreme(&self.values, true),
        })
    }
}

/// M2c spec §4 and amendment 1: equal numerics order `Int`, then `Real`,
/// then `Real(-0.0)`; MIN outputs the first, MAX the last.
fn tie_rank(v: &Value) -> u8 {
    match v {
        Value::Real(x) if *x == 0.0 && x.is_sign_negative() => 2,
        Value::Real(_) => 1,
        _ => 0,
    }
}

/// MIN (`max = false`) or MAX of `values` in M2c spec §2's order; NULL when empty.
fn extreme(values: &[Value], max: bool) -> Value {
    let order = |a: &&Value, b: &&Value| a.sql_cmp(b).then(tie_rank(a).cmp(&tie_rank(b)));
    let it = values.iter();
    (if max {
        it.max_by(order)
    } else {
        it.min_by(order)
    })
    .cloned()
    .unwrap_or(Value::Null)
}

impl Engine for NaiveRecompute {
    fn create_view(
        &mut self,
        db: &Database,
        query: &ViewQuery,
        initial: &BTreeMap<String, ZSet>,
    ) -> Result<(), EngineError> {
        // Resolve everything that can fail before touching `self`, then commit
        // the whole new view at once.
        joined_columns(db, query)?;
        self.view = Some((db.clone(), query.clone()));
        self.base = initial.clone();
        // Deltas applied but not yet refreshed belong to the view being replaced.
        self.pending.clear();
        Ok(())
    }

    fn apply(&mut self, table: &str, raw: &[(Row, i64)]) -> Result<(), EngineError> {
        // Deliberately unmerged: merging is refresh's job; see the type's doc comment.
        self.pending.extend(
            raw.iter()
                .map(|(row, w)| (table.to_string(), row.clone(), *w)),
        );
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), EngineError> {
        for (table, row, weight) in self.pending.drain(..) {
            self.base.entry(table).or_default().update(row, weight);
        }
        Ok(())
    }

    fn materialize(&mut self) -> Result<ZSet, EngineError> {
        let (db, query) = self
            .view
            .as_ref()
            .ok_or_else(|| EngineError("materialize was called before create_view".into()))?;
        Ok(evaluate(db, query, &self.base, &self.terms)?.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{join_on_k, kv, single_table_bases as single_table_case};
    use crate::{Agg, Schema};

    fn schema() -> Schema {
        Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
            ],
        }
    }

    // A single-table case wrapped as a one-table `Database`, with initial state
    // keyed by table name — every existing single-table test cares only about
    // this one anchor table, and should stay this terse after the multi-table
    // change. This uses `test_support::single_table_bases` (m4: the
    // byte-identical version in buggy.rs was merged into it).

    fn sum_by_region() -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        }
    }

    fn row(region: &str, amount: i64) -> Row {
        Row::new(vec![Value::Text(region.into()), Value::Int(amount)])
    }

    fn out(region: Value, sum: i64, count: i64) -> Row {
        Row::new(vec![region, Value::Int(sum), Value::Int(count)])
    }

    #[test]
    fn joins_on_the_key_and_aggregates_the_joined_rows() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: &str, v: i64| Row::new(vec![Value::Text(k.into()), Value::Int(v)]);
        let bases = BTreeMap::from([
            (
                "t0".to_string(),
                ZSet::from_rows([(r("a", 1), 1), (r("b", 2), 1)]),
            ),
            (
                "t1".to_string(),
                ZSet::from_rows([(r("a", 10), 1), (r("a", 20), 1)]),
            ),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Int(2),
                Value::Int(30)
            ])),
            1
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn null_join_keys_never_match() {
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let null_key = |v: i64| Row::new(vec![Value::Null, Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(null_key(1), 1)])),
            ("t1".to_string(), ZSet::from_rows([(null_key(10), 1)])),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        assert!(e.materialize().unwrap().is_empty());
    }

    #[test]
    fn two_negative_rows_do_not_join_into_a_positive_one() {
        // Spec §5.1: a row with weight ≤ 0 takes no part in recomputation. Two
        // retractions of rows never inserted leave `("a", 2)` at -1 in t0 and
        // `("a", 10)` at -1 in t1; their product is +1, so skipping
        // non-positive weights only after joining would report a group "a".
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: &str, v: i64| Row::new(vec![Value::Text(k.into()), Value::Int(v)]);
        let bases = BTreeMap::from([
            ("t0".to_string(), ZSet::from_rows([(r("a", 1), 1)])),
            ("t1".to_string(), ZSet::new()),
        ]);
        let mut e = NaiveRecompute::new();
        e.create_view(&db, &join_on_k(), &bases).unwrap();
        e.apply("t0", &[(r("a", 2), -1)]).unwrap();
        e.apply("t1", &[(r("a", 10), -1)]).unwrap();
        e.refresh().unwrap();
        let got = e.materialize().unwrap();
        assert!(
            got.is_empty(),
            "t1 holds no row of positive weight, so the join is empty: {got:?}"
        );
    }

    #[test]
    fn recreating_a_view_discards_deltas_applied_but_not_refreshed() {
        // A reference engine reused across `create_view` calls must not carry
        // unrefreshed deltas into the new view. `create_view` replaces the query
        // and the base state, so anything still pending belongs to the old view:
        // letting it reach the next `refresh` would pollute every comparison the
        // reference engine is then used for. `IncrementalEngine` already gets this
        // right; this pins the reference engine to the same contract.
        let mut e = NaiveRecompute::new();
        let (db, bases) = single_table_case(&schema(), ZSet::new());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();
        e.apply("orders", &[(row("stale", 99), 1)]).unwrap();

        // Re-create the view from empty initial state, then refresh.
        let (db, bases) = single_table_case(&schema(), ZSet::new());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert!(
            got.is_empty(),
            "a delta applied before the view was re-created leaked into it: {got:?}"
        );
    }

    #[test]
    fn aggregates_initial_state() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1), (row("b", 3), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 15, 2)), 1);
        assert_eq!(got.weight_of(&out(Value::Text("b".into()), 3, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn applying_a_delete_updates_the_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 5), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        e.apply("orders", &[(row("a", 5), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Text("a".into()), 10, 1)), 1);
        assert_eq!(got.len(), 1, "the old (a,15,2) must disappear");
    }

    #[test]
    fn emptying_a_group_removes_it_entirely() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        e.apply("orders", &[(row("a", 10), -1)]).unwrap();
        e.refresh().unwrap();

        assert!(
            e.materialize().unwrap().is_empty(),
            "an empty group must leave no zombie row"
        );
    }

    #[test]
    fn null_forms_its_own_group() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Null, Value::Int(4)]), 1),
            (row("a", 1), 1),
        ]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(got.weight_of(&out(Value::Null, 4, 1)), 1);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn predicate_filters_before_aggregating() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::Compare {
                column: 1,
                op: CmpOp::Gt,
                value: Value::Int(4),
            },
            join: None,
        };
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", 1), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &q, &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
    }

    #[test]
    fn naive_passes_follows_three_valued_logic_for_not_equal_and_is_null() {
        let int = column("x", ColumnType::Integer);
        let columns = [&int];
        let ne = Predicate::Compare {
            column: 0,
            op: CmpOp::Ne,
            value: Value::Int(4),
        };
        assert!(passes(&ne, &Row::new(vec![Value::Int(3)]), &columns));
        assert!(!passes(&ne, &Row::new(vec![Value::Int(4)]), &columns));
        assert!(
            !passes(&ne, &Row::new(vec![Value::Null]), &columns),
            "NULL != 4 is UNKNOWN"
        );
        let is_null = Predicate::IsNull { column: 0 };
        assert!(passes(&is_null, &Row::new(vec![Value::Null]), &columns));
        assert!(!passes(&is_null, &Row::new(vec![Value::Int(0)]), &columns));
    }

    fn column(name: &str, ty: ColumnType) -> Column {
        Column {
            name: name.into(),
            ty,
            nullable: true,
        }
    }

    /// `t(k TEXT, x <ty>)`.
    fn kx(ty: ColumnType) -> Schema {
        Schema {
            table: "t".into(),
            columns: vec![column("k", ColumnType::Text), column("x", ty)],
        }
    }

    fn by_k(aggs: Vec<(AggFn, Option<usize>)>) -> ViewQuery {
        ViewQuery {
            group_by: vec![0],
            aggs: aggs
                .into_iter()
                .map(|(func, column)| Agg { func, column })
                .collect(),
            predicate: Predicate::None,
            join: None,
        }
    }

    fn evaluate_rows(
        schema: &Schema,
        query: &ViewQuery,
        rows: Vec<Row>,
    ) -> Result<Evaluated, EngineError> {
        let (db, bases) =
            single_table_case(schema, ZSet::from_rows(rows.into_iter().map(|r| (r, 1))));
        evaluate(&db, query, &bases, &SqliteTerms::new().unwrap())
    }

    fn kx_row(k: &str, x: Value) -> Row {
        Row::new(vec![Value::Text(k.into()), x])
    }

    #[test]
    fn affinity_literal_follows_sqlites_comparison_rules() {
        // M2c spec §5, with the literal in its comparison form.
        for ty in [ColumnType::Integer, ColumnType::Real, ColumnType::Numeric] {
            let lit = |t: &str| affinity_literal(ty, &Value::Text(t.into()));
            assert_eq!(lit("3.0"), Value::Int(3));
            assert_eq!(lit("2.5e1"), Value::Int(25));
            assert_eq!(lit("-0.0"), Value::Int(0));
            assert_eq!(lit("2.5"), Value::Real(2.5));
            assert_eq!(lit("x"), Value::Text("x".into()));
            assert_eq!(affinity_literal(ty, &Value::Int(3)), Value::Int(3));
        }
        assert_eq!(
            affinity_literal(ColumnType::Text, &Value::Int(12)),
            Value::Text("12".into())
        );
        assert_eq!(
            affinity_literal(ColumnType::Blob, &Value::Text("3".into())),
            Value::Text("3".into())
        );
        assert_eq!(
            affinity_literal(ColumnType::Text, &Value::Blob(vec![1])),
            Value::Blob(vec![1])
        );
    }

    #[test]
    fn a_text_literal_meets_an_integer_column_as_a_number_as_in_sqlite() {
        let q = ViewQuery {
            predicate: Predicate::Compare {
                column: 1,
                op: CmpOp::Ge,
                value: Value::Text("3.0".into()),
            },
            ..by_k(vec![(AggFn::Count, None)])
        };
        let s = kx(ColumnType::Integer);
        let rows: Vec<Row> = (1..=4).map(|n| kx_row("a", Value::Int(n))).collect();
        let got = evaluate_rows(&s, &q, rows.clone()).unwrap();
        assert_eq!(got.rows, ZSet::from_rows([(kx_row("a", Value::Int(2)), 1)]));
        let (db, bases) = single_table_case(&s, ZSet::from_rows(rows.into_iter().map(|r| (r, 1))));
        assert_eq!(
            crate::recompute_via_sqlite(&db, &q, &bases).unwrap(),
            got.rows
        );
    }

    #[test]
    fn sqlite_classifies_each_term() {
        // M2c spec §4, read from SQLite itself.
        let terms = SqliteTerms::new().unwrap();
        let int = |v: Value| match terms.term(&v).unwrap() {
            SqliteTerm::Integer(n) => Some(n),
            SqliteTerm::Approx(_) => None,
        };
        let approx = |v: Value| match terms.term(&v).unwrap() {
            SqliteTerm::Approx(x) => Some(x.to_bits()),
            SqliteTerm::Integer(_) => None,
        };
        assert_eq!(int(Value::Int(i64::MAX)), Some(i64::MAX));
        assert_eq!(int(Value::Text("12".into())), Some(12));
        assert_eq!(approx(Value::Text("1.5".into())), Some(1.5f64.to_bits()));
        assert_eq!(approx(Value::Text("3.0".into())), Some(3.0f64.to_bits()));
        assert_eq!(approx(Value::Text("abc".into())), Some(0.0f64.to_bits()));
        assert_eq!(approx(Value::Real(2.0)), Some(2.0f64.to_bits()));
        assert_eq!(approx(Value::Blob(b"12".to_vec())), Some(12.0f64.to_bits()));
        assert_eq!(
            approx(Value::Text("9223372036854775808".into())),
            Some(9_223_372_036_854_775_808.0f64.to_bits())
        );
        // A second ask is served from the cache, unchanged.
        assert_eq!(int(Value::Text("12".into())), Some(12));
        assert_eq!(terms.cache.borrow().len(), 8);
    }

    #[test]
    fn real_sums_and_means_are_rounded_once() {
        let q = by_k(vec![
            (AggFn::Sum, Some(1)),
            (AggFn::Total, Some(1)),
            (AggFn::Avg, Some(1)),
            (AggFn::Count, Some(1)),
        ]);
        let rows = vec![
            kx_row("a", Value::Real(0.1)),
            kx_row("a", Value::Real(0.2)),
            kx_row("a", Value::Real(0.3)),
            kx_row("a", Value::Null),
        ];
        let got = evaluate_rows(&kx(ColumnType::Real), &q, rows).unwrap();
        let want = Row::new(vec![
            Value::Text("a".into()),
            Value::Real(0.6),
            Value::Real(0.6),
            Value::Real(0.2),
            Value::Int(3),
        ]);
        assert_eq!(got.rows, ZSet::from_rows([(want, 1)]));
    }

    #[test]
    fn a_mix_of_integer_and_real_terms_sums_exactly() {
        // 2^53 + 1 is an INTEGER term; adding 0.5 makes the group approximate,
        // and the exact 2^53 + 1.5 rounds to 2^53 + 2.
        let q = by_k(vec![(AggFn::Sum, Some(1)), (AggFn::Avg, Some(1))]);
        let p53 = 2f64.powi(53);
        let rows = vec![
            kx_row("a", Value::Int((1 << 53) + 1)),
            kx_row("a", Value::Real(0.5)),
        ];
        let got = evaluate_rows(&kx(ColumnType::Blob), &q, rows).unwrap();
        let want = Row::new(vec![
            Value::Text("a".into()),
            Value::Real(p53 + 2.0),
            // (2^53 + 1.5) / 2 = 2^52 + 0.75, which rounds to 2^52 + 1.
            Value::Real(p53 / 2.0 + 1.0),
        ]);
        assert_eq!(got.rows, ZSet::from_rows([(want, 1)]));
    }

    #[test]
    fn the_kbn_magnitude_counts_every_term_by_weight() {
        let q = by_k(vec![(AggFn::Total, Some(1)), (AggFn::Min, Some(1))]);
        let s = kx(ColumnType::Blob);
        let (db, bases) = single_table_case(
            &s,
            ZSet::from_rows([
                (kx_row("a", Value::Int(3)), 2),
                (kx_row("a", Value::Int(-5)), 1),
                (kx_row("a", Value::Real(-0.5)), 3),
            ]),
        );
        let got = evaluate(&db, &q, &bases, &SqliteTerms::new().unwrap()).unwrap();
        let key = Row::new(vec![Value::Text("a".into())]);
        assert_eq!(got.magnitudes, BTreeMap::from([((key, 0), (12.5, 6))]));
    }

    #[test]
    fn an_integer_sum_outside_i64_overflows_but_total_does_not() {
        let rows = vec![
            kx_row("a", Value::Int(i64::MAX)),
            kx_row("b", Value::Int(1)),
        ];
        let mut rows_a = rows.clone();
        rows_a.push(kx_row("a", Value::Int(1)));
        let s = kx(ColumnType::Integer);
        assert_eq!(
            evaluate_rows(&s, &by_k(vec![(AggFn::Sum, Some(1))]), rows_a.clone()).unwrap_err(),
            EngineError(SUM_OVERFLOW.into())
        );
        let total = evaluate_rows(&s, &by_k(vec![(AggFn::Total, Some(1))]), rows_a).unwrap();
        assert_eq!(
            total
                .rows
                .weight_of(&kx_row("a", Value::Real(2f64.powi(63)))),
            1
        );
        assert!(evaluate_rows(&s, &by_k(vec![(AggFn::Sum, Some(1))]), rows).is_ok());
    }

    #[test]
    fn infinities_are_counted_outside_the_sum() {
        // M2c spec §4, amendment 5.
        let q = by_k(vec![(AggFn::Sum, Some(1)), (AggFn::Avg, Some(1))]);
        let s = kx(ColumnType::Real);
        let one = evaluate_rows(
            &s,
            &q,
            vec![
                kx_row("a", Value::Real(f64::INFINITY)),
                kx_row("a", Value::Real(1.0)),
            ],
        )
        .unwrap();
        let inf = Value::Real(f64::INFINITY);
        assert_eq!(
            one.rows,
            ZSet::from_rows([(Row::new(vec![Value::Text("a".into()), inf.clone(), inf]), 1)])
        );
        let both = evaluate_rows(
            &s,
            &q,
            vec![
                kx_row("a", Value::Real(f64::INFINITY)),
                kx_row("a", Value::Real(f64::NEG_INFINITY)),
            ],
        )
        .unwrap();
        assert_eq!(
            both.rows,
            ZSet::from_rows([(
                Row::new(vec![Value::Text("a".into()), Value::Null, Value::Null]),
                1
            )])
        );
    }

    #[test]
    fn min_and_max_break_numeric_ties_by_class() {
        // M2c spec §4 and amendment 1: Int < Real < Real(-0.0) among equals.
        let q = by_k(vec![(AggFn::Min, Some(1)), (AggFn::Max, Some(1))]);
        let s = kx(ColumnType::Blob);
        let zeros = vec![
            kx_row("a", Value::Real(-0.0)),
            kx_row("a", Value::Real(0.0)),
            kx_row("a", Value::Int(0)),
        ];
        let got = evaluate_rows(&s, &q, zeros).unwrap();
        let want = Row::new(vec![
            Value::Text("a".into()),
            Value::Int(0),
            Value::Real(-0.0),
        ]);
        assert_eq!(got.rows, ZSet::from_rows([(want, 1)]));
        // Across classes: numeric < TEXT < BLOB; NULL is skipped.
        let mixed = vec![
            kx_row("a", Value::Blob(vec![0])),
            kx_row("a", Value::Text("z".into())),
            kx_row("a", Value::Real(2.5)),
            kx_row("a", Value::Null),
        ];
        let got = evaluate_rows(&s, &q, mixed).unwrap();
        let want = Row::new(vec![
            Value::Text("a".into()),
            Value::Real(2.5),
            Value::Blob(vec![0]),
        ]);
        assert_eq!(got.rows, ZSet::from_rows([(want, 1)]));
    }

    #[test]
    fn a_group_keyed_by_one_and_one_point_zero_is_one_real_keyed_group() {
        // M2c spec §2: canonical keys; output as REAL when a row's key was REAL.
        let s = Schema {
            table: "t".into(),
            columns: vec![column("x", ColumnType::Blob)],
        };
        let q = ViewQuery {
            group_by: vec![0],
            ..by_k(vec![(AggFn::Count, None)])
        };
        let rows = |keys: Vec<Value>| keys.into_iter().map(|k| Row::new(vec![k])).collect();
        let got = evaluate_rows(
            &s,
            &q,
            rows(vec![
                Value::Int(1),
                Value::Real(1.0),
                Value::Text("1".into()),
            ]),
        )
        .unwrap();
        assert_eq!(
            got.rows,
            ZSet::from_rows([
                (Row::new(vec![Value::Real(1.0), Value::Int(2)]), 1),
                (Row::new(vec![Value::Text("1".into()), Value::Int(1)]), 1),
            ])
        );
        let got = evaluate_rows(&s, &q, rows(vec![Value::Real(-0.0), Value::Int(0)])).unwrap();
        assert_eq!(
            got.rows,
            ZSet::from_rows([(Row::new(vec![Value::Real(0.0), Value::Int(2)]), 1)])
        );
    }

    #[test]
    fn join_keys_match_by_canonical_value() {
        // M2c spec §5: 1 matches 1.0; NULL matches nothing.
        let db = Database::new(vec![kv("t0"), kv("t1")]);
        let r = |k: Value, v: i64| Row::new(vec![k, Value::Int(v)]);
        let bases = BTreeMap::from([
            (
                "t0".to_string(),
                ZSet::from_rows([(r(Value::Int(1), 1), 1), (r(Value::Null, 2), 1)]),
            ),
            (
                "t1".to_string(),
                ZSet::from_rows([(r(Value::Real(1.0), 10), 1), (r(Value::Null, 20), 1)]),
            ),
        ]);
        let got = evaluate(&db, &join_on_k(), &bases, &SqliteTerms::new().unwrap()).unwrap();
        assert_eq!(
            got.rows,
            ZSet::from_rows([(
                Row::new(vec![Value::Int(1), Value::Int(1), Value::Int(10)]),
                1
            )])
        );
    }

    /// Item 13 (a deferred minor, from the same source as I4): the `IsNotNull`
    /// predicate used to have no behavioural coverage — only its `to_sql`
    /// rendering, with nothing asserting it actually filters NULL rows out.
    #[test]
    fn is_not_null_predicate_filters_out_null_rows() {
        let mut e = NaiveRecompute::new();
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![Agg {
                func: AggFn::Count,
                column: None,
            }],
            predicate: Predicate::IsNotNull { column: 0 },
            join: None,
        };
        let base = ZSet::from_rows([
            (row("a", 10), 1),
            (Row::new(vec![Value::Null, Value::Int(1)]), 1),
        ]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &q, &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&Row::new(vec![Value::Text("a".into()), Value::Int(1)])),
            1
        );
        assert_eq!(
            got.len(),
            1,
            "the NULL group must be filtered out by IsNotNull and not appear in the output"
        );
    }

    /// Spec §6.1's NULL-semantics contract. Note this differs from "the group is
    /// empty": the group is non-empty (COUNT(*) is positive) but the summed
    /// column is entirely NULL, and then SUM is NULL.
    #[test]
    fn sum_over_all_null_column_is_null_not_zero() {
        let nullable_amount = Schema {
            table: "orders".into(),
            columns: vec![
                Column {
                    name: "region".into(),
                    ty: ColumnType::Text,
                    nullable: false,
                },
                Column {
                    name: "amount".into(),
                    ty: ColumnType::Integer,
                    nullable: true,
                },
            ],
        };
        let q = ViewQuery {
            group_by: vec![0],
            aggs: vec![
                Agg {
                    func: AggFn::Sum,
                    column: Some(1),
                },
                Agg {
                    func: AggFn::Count,
                    column: None,
                },
            ],
            predicate: Predicate::None,
            join: None,
        };
        // The two identical rows are merged by the ZSet into weight 2
        let base = ZSet::from_rows([
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
            (Row::new(vec![Value::Text("a".into()), Value::Null]), 1),
        ]);

        let mut e = NaiveRecompute::new();
        let (db, bases) = single_table_case(&nullable_amount, base.clone());
        e.create_view(&db, &q, &bases).unwrap();
        let got = e.materialize().unwrap();

        assert_eq!(
            got.weight_of(&Row::new(vec![
                Value::Text("a".into()),
                Value::Null,
                Value::Int(2)
            ])),
            1,
            "SUM with no non-NULL input should be NULL, while COUNT(*) is still 2"
        );
    }

    /// Complements the previous test: here non-NULL inputs exist (and are
    /// non-zero), they just sum to exactly 0. If the emit branch's test were
    /// wrongly changed from `non_null == 0` to `total == 0`, this test would
    /// fail and no other would.
    #[test]
    fn sum_that_totals_zero_is_int_zero_not_null() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1), (row("a", -10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 0, 2)),
            1,
            "non-NULL inputs summing to exactly 0 should output Int(0), not Null"
        );
    }

    /// A pure retraction of a row never inserted (applying a delta of weight -1)
    /// leaves it in self.base with a negative weight. If materialize's
    /// `weight <= 0` guard were deleted, that negative-weight row would be
    /// aggregated as a real input row, and no other test would notice — both
    /// "delete" tests insert first and then retract to exactly 0, and the ZSet
    /// removes a row the moment its weight reaches zero, so materialize never
    /// even visits it.
    #[test]
    fn retracting_a_row_that_was_never_inserted_is_a_noop() {
        let mut e = NaiveRecompute::new();
        let base = ZSet::from_rows([(row("a", 10), 1)]);
        let (db, bases) = single_table_case(&schema(), base.clone());
        e.create_view(&db, &sum_by_region(), &bases).unwrap();

        // "b" never appeared in base; this retraction leaves it at weight -1 in self.base.
        e.apply("orders", &[(row("b", 999), -1)]).unwrap();
        e.refresh().unwrap();

        let got = e.materialize().unwrap();
        assert_eq!(
            got.weight_of(&out(Value::Text("a".into()), 10, 1)),
            1,
            "an unaffected group must stay unchanged"
        );
        assert_eq!(
            got.len(),
            1,
            "a phantom negative-weight row must produce no output and not affect other groups"
        );
    }
}
