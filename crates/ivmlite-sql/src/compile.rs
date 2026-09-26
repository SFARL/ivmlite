use ivmlite_core::{
    lower, Agg, AggFn, CmpOp, Column, ColumnType, Plan, PlanError, Predicate, ResolvedJoin,
    ResolvedView, Schema, Value,
};
use sqlparser::ast::{
    BinaryOperator, Distinct, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgumentList,
    FunctionArguments, GroupByExpr, Ident, Join, JoinConstraint, JoinOperator, ObjectName,
    ObjectNamePart, Query, Select, SelectFlavor, SelectItem, SetExpr, Statement, TableAlias,
    TableFactor, TableWithJoins, UnaryOperator, Value as SqlValue,
};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::source::select_list_pieces;
use crate::{Catalog, CatalogError};

/// A view's SQL, compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledView {
    pub plan: Plan,
    /// The view's output columns in SELECT order — its group-by columns, then
    /// its aggregates — with the names SQLite would give them.
    pub columns: Vec<Column>,
    /// The base tables the view reads, the anchor (the FROM table) first:
    /// the view's rows in `__ivm_dep` (spec §7).
    pub tables: Vec<String>,
}

/// Why a view's SQL was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlError(pub String);

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SqlError {}

impl From<PlanError> for SqlError {
    fn from(e: PlanError) -> Self {
        SqlError(e.0)
    }
}

impl From<CatalogError> for SqlError {
    fn from(e: CatalogError) -> Self {
        SqlError(e.0)
    }
}

fn unsupported(what: &str) -> SqlError {
    SqlError(format!(
        "{what} is not supported in an ivmlite v0 view (spec §13)"
    ))
}

fn reject_if(present: bool, what: &str) -> Result<(), SqlError> {
    if present {
        Err(unsupported(what))
    } else {
        Ok(())
    }
}

/// Compile a view's `SELECT` against `catalog`.
///
/// v0's subset (spec §5.2, §6.1, §13):
///
/// ```text
/// SELECT <column>, …, <aggregate>, …
/// FROM <table> [[AS] <alias>]
///      [[INNER] JOIN <table> [[AS] <alias>] ON <column> = <column>]
/// [WHERE <column> <op> <literal> | <literal> <op> <column>
///      | <column> IS NULL | <column> IS NOT NULL]
/// GROUP BY <column>, …
/// ```
///
/// where the SELECT list's bare columns are exactly the GROUP BY columns and
/// come before the aggregates, an aggregate is `COUNT(*)` or `SUM(<column>)`,
/// `<op>` is one of `>` `>=` `<` `<=` `=` `!=` `<>`, and a literal is an
/// integer or a single-quoted string. Anything else is an error.
pub fn compile(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError> {
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| SqlError(format!("cannot parse the view's SQL: {e}")))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(unsupported("anything but exactly one SELECT statement"));
    };
    let select = plain_select(query)?;
    let (anchor, join) = from_clause(select, catalog)?;
    let scope = Scope::new(&anchor, join.as_ref().map(|j| &j.table));
    let join = match &join {
        None => None,
        Some(j) => Some(join_on(&scope, j)?),
    };
    let outputs = projection(sql, &scope, &select.projection)?;
    let group_by = group_by(&scope, &select.group_by, &outputs)?;
    let predicate = match &select.selection {
        None => Predicate::None,
        Some(e) => predicate(&scope, e)?,
    };
    let aggs = outputs
        .iter()
        .filter_map(|o| match &o.item {
            Output::Agg(agg) => Some(agg.clone()),
            Output::Column(_) => None,
        })
        .collect();
    let plan = lower(&ResolvedView {
        anchor: &anchor.schema,
        join,
        group_by,
        aggs,
        predicate,
    })?;
    let columns = outputs.iter().map(|o| o.column(&scope)).collect();
    let mut tables = vec![anchor.schema.table.clone()];
    tables.extend(scope.right.map(|r| r.schema.table.clone()));
    Ok(CompiledView {
        plan,
        columns,
        tables,
    })
}

/// A table in the FROM clause, and the name the query uses for it.
struct Source {
    schema: Schema,
    /// The alias if there is one, else the table's name as written.
    qualifier: String,
}

/// The JOIN clause, before its ON condition is resolved.
struct JoinClause<'a> {
    table: Source,
    on: &'a Expr,
}

/// The tables a query's column names resolve against. Column `i` of the
/// joined row is the anchor's column `i`, then the right table's.
struct Scope<'a> {
    anchor: &'a Source,
    right: Option<&'a Source>,
}

impl<'a> Scope<'a> {
    fn new(anchor: &'a Source, right: Option<&'a Source>) -> Self {
        Scope { anchor, right }
    }

    fn sources(&self) -> impl Iterator<Item = (&'a Source, usize)> {
        let offset = self.anchor.schema.arity();
        std::iter::once((self.anchor, 0)).chain(self.right.map(|r| (r, offset)))
    }

    fn column_at(&self, index: usize) -> &'a Column {
        let n = self.anchor.schema.arity();
        if index < n {
            &self.anchor.schema.columns[index]
        } else {
            &self
                .right
                .expect("an index past the anchor's columns belongs to the right table")
                .schema
                .columns[index - n]
        }
    }

    /// Resolve a column reference to its index in the joined row.
    fn column(&self, expr: &Expr) -> Result<Option<usize>, SqlError> {
        match strip_parens(expr) {
            Expr::Identifier(name) => {
                let mut found = self
                    .sources()
                    .filter_map(|(s, offset)| position(&s.schema, &name.value).map(|i| offset + i));
                match (found.next(), found.next()) {
                    (Some(i), None) => Ok(Some(i)),
                    (None, _) => Err(SqlError(format!(
                        "no such column: {}{}",
                        name.value,
                        // Final review Minor 4: SQLite falls back to reading
                        // a double-quoted token as a string literal when it
                        // does not name a column; v0 does not, so say so
                        // rather than leave it looking like a typo.
                        if name.quote_style == Some('"') {
                            " (a string literal takes single quotes, not double quotes)"
                        } else {
                            ""
                        }
                    ))),
                    (Some(_), Some(_)) => Err(SqlError(format!(
                        "ambiguous column name: {} — qualify it with its table",
                        name.value
                    ))),
                }
            }
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [table, name] => {
                    let (source, offset) = self
                        .sources()
                        .find(|(s, _)| s.qualifier.eq_ignore_ascii_case(&table.value))
                        .ok_or_else(|| SqlError(format!("no such table: {}", table.value)))?;
                    let i = position(&source.schema, &name.value).ok_or_else(|| {
                        SqlError(format!("no such column: {}.{}", table.value, name.value))
                    })?;
                    Ok(Some(offset + i))
                }
                _ => Err(unsupported("a column name with a schema prefix")),
            },
            _ => Ok(None),
        }
    }

    /// Like `column`, but anything other than a column reference is an error.
    fn require_column(&self, expr: &Expr, context: &str) -> Result<usize, SqlError> {
        self.column(expr)?.ok_or_else(|| {
            unsupported(&format!(
                "{context} `{expr}` (only a bare column is allowed there)"
            ))
        })
    }
}

fn position(schema: &Schema, name: &str) -> Option<usize> {
    schema
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(name))
}

fn strip_parens(mut expr: &Expr) -> &Expr {
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    expr
}

/// The query's single SELECT, with every clause v0 does not support rejected.
///
/// `Query` and `Select` are destructured without `..`, so a field a future
/// `sqlparser` adds is a compile error here rather than a clause that is
/// silently ignored.
fn plain_select(query: &Query) -> Result<&Select, SqlError> {
    let Query {
        with,
        body,
        order_by,
        limit_clause,
        fetch,
        locks,
        for_clause,
        settings,
        format_clause,
        pipe_operators,
    } = query;
    reject_if(with.is_some(), "WITH")?;
    reject_if(order_by.is_some(), "ORDER BY")?;
    reject_if(limit_clause.is_some() || fetch.is_some(), "LIMIT")?;
    reject_if(
        !locks.is_empty()
            || for_clause.is_some()
            || settings.is_some()
            || format_clause.is_some()
            || !pipe_operators.is_empty(),
        "a non-SQLite query clause",
    )?;
    let SetExpr::Select(select) = body.as_ref() else {
        return Err(unsupported(
            "a compound or parenthesized query (UNION, INTERSECT, EXCEPT, VALUES)",
        ));
    };
    let Select {
        select_token: _,
        optimizer_hints,
        distinct,
        select_modifiers,
        top,
        top_before_distinct: _,
        projection: _,
        exclude,
        into,
        from: _,
        lateral_views,
        prewhere,
        selection: _,
        connect_by,
        group_by: _,
        cluster_by,
        distribute_by,
        sort_by,
        having,
        named_window,
        qualify,
        window_before_qualify: _,
        value_table_mode,
        flavor,
    } = select.as_ref();
    // `ALL` (or no modifier at all) is a plain SELECT to SQLite; only an
    // actual DISTINCT is rejected (final review Minor 3).
    reject_if(
        matches!(distinct, Some(Distinct::Distinct) | Some(Distinct::On(_))),
        "DISTINCT",
    )?;
    reject_if(having.is_some(), "HAVING")?;
    reject_if(!named_window.is_empty(), "WINDOW")?;
    reject_if(
        !optimizer_hints.is_empty()
            || select_modifiers.is_some()
            || top.is_some()
            || exclude.is_some()
            || into.is_some()
            || !lateral_views.is_empty()
            || prewhere.is_some()
            || !connect_by.is_empty()
            || !cluster_by.is_empty()
            || !distribute_by.is_empty()
            || !sort_by.is_empty()
            || qualify.is_some()
            || value_table_mode.is_some()
            || !matches!(flavor, SelectFlavor::Standard),
        "a non-SQLite SELECT clause",
    )?;
    Ok(select)
}

/// The FROM clause: the anchor table and, optionally, one inner join.
fn from_clause<'a>(
    select: &'a Select,
    catalog: &dyn Catalog,
) -> Result<(Source, Option<JoinClause<'a>>), SqlError> {
    let [TableWithJoins { relation, joins }] = select.from.as_slice() else {
        return Err(if select.from.is_empty() {
            unsupported("a SELECT without FROM")
        } else {
            unsupported("a comma-separated FROM list (write JOIN … ON instead)")
        });
    };
    let anchor = source(relation, catalog)?;
    let join = match joins.as_slice() {
        [] => None,
        [Join {
            relation,
            global,
            join_operator,
        }] => {
            // `FROM t0 GLOBAL JOIN t1`: sqlparser reads a GLOBAL join, but
            // SQLite reads `GLOBAL` as `t0`'s alias (measured, 3.53).
            reject_if(*global, "GLOBAL JOIN")?;
            let constraint =
                match join_operator {
                    JoinOperator::Join(c) | JoinOperator::Inner(c) => c,
                    _ => return Err(unsupported(
                        "an outer, cross or other non-inner join (v0 joins with INNER JOIN … ON)",
                    )),
                };
            let JoinConstraint::On(on) = constraint else {
                return Err(unsupported("a join without ON (USING, NATURAL, or none)"));
            };
            let table = source(relation, catalog)?;
            if table.qualifier.eq_ignore_ascii_case(&anchor.qualifier) {
                return Err(SqlError(format!(
                    "the name {} is used for both tables of the join; give them different aliases",
                    table.qualifier
                )));
            }
            Some(JoinClause { table, on })
        }
        _ => return Err(unsupported("a join of more than two tables")),
    };
    Ok((anchor, join))
}

fn source(relation: &TableFactor, catalog: &dyn Catalog) -> Result<Source, SqlError> {
    let TableFactor::Table {
        name,
        alias,
        args,
        with_hints,
        version,
        with_ordinality,
        partitions,
        json_path,
        sample,
        index_hints,
    } = relation
    else {
        return Err(unsupported("a subquery or table function in FROM"));
    };
    reject_if(
        args.is_some()
            || !with_hints.is_empty()
            || version.is_some()
            || *with_ordinality
            || !partitions.is_empty()
            || json_path.is_some()
            || sample.is_some()
            || !index_hints.is_empty(),
        "a table modifier in FROM",
    )?;
    let table_name = single_ident(name)?;
    let schema = catalog
        .table(&table_name.value)?
        .ok_or_else(|| SqlError(format!("no such table: {}", table_name.value)))?;
    let qualifier = match alias {
        None => table_name.value.clone(),
        Some(TableAlias {
            explicit: _,
            name,
            columns,
            at,
        }) => {
            reject_if(
                !columns.is_empty() || at.is_some(),
                "a table alias with a column list",
            )?;
            name.value.clone()
        }
    };
    Ok(Source { schema, qualifier })
}

fn single_ident(name: &ObjectName) -> Result<&Ident, SqlError> {
    match name.0.as_slice() {
        [ObjectNamePart::Identifier(ident)] => Ok(ident),
        _ => Err(unsupported("a table name with a schema prefix")),
    }
}

/// `ON <column> = <column>`, one column from each table, in either order.
fn join_on<'a>(scope: &Scope<'a>, join: &JoinClause) -> Result<ResolvedJoin<'a>, SqlError> {
    let right = scope.right.expect("a join has a right table");
    let not_supported = || {
        unsupported(&format!(
            "the join condition `{}` (v0 joins on one equality between a column of each table)",
            join.on
        ))
    };
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right: other,
    } = strip_parens(join.on)
    else {
        return Err(not_supported());
    };
    let (Some(a), Some(b)) = (scope.column(left)?, scope.column(other)?) else {
        return Err(not_supported());
    };
    let n = scope.anchor.schema.arity();
    let (left_column, right_column) = match (a < n, b < n) {
        (true, false) => (a, b - n),
        (false, true) => (b, a - n),
        _ => return Err(not_supported()),
    };
    Ok(ResolvedJoin {
        right: &right.schema,
        left_column,
        right_column,
    })
}

enum Output {
    Column(usize),
    Agg(Agg),
}

/// One SELECT-list entry and the name SQLite gives it.
struct Named {
    item: Output,
    name: String,
}

impl Named {
    fn column(&self, scope: &Scope) -> Column {
        match &self.item {
            &Output::Column(i) => Column {
                name: self.name.clone(),
                ..scope.column_at(i).clone()
            },
            Output::Agg(agg) => Column {
                name: self.name.clone(),
                ty: ColumnType::Integer,
                // SUM over only NULLs is NULL (spec §6.1); COUNT(*) is never NULL.
                nullable: agg.func == AggFn::Sum,
            },
        }
    }
}

/// The SELECT list: bare columns first, then aggregates, each named the way
/// SQLite names a result column — its alias, else a bare column's declared
/// name, else an aggregate's text as written.
fn projection(sql: &str, scope: &Scope, items: &[SelectItem]) -> Result<Vec<Named>, SqlError> {
    let pieces = select_list_pieces(sql, items.len())?;
    let mut out: Vec<Named> = Vec::new();
    for (sel_item, piece) in items.iter().zip(pieces) {
        let (expr, alias) = match sel_item {
            SelectItem::UnnamedExpr(expr) => (expr, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias)),
            _ => return Err(unsupported("SELECT * and multi-name aliases")),
        };
        let (item, default_name) = match scope.column(expr)? {
            Some(i) => {
                if out.iter().any(|o| matches!(o.item, Output::Agg(_))) {
                    return Err(unsupported(
                        "a column after an aggregate in the SELECT list (list the GROUP BY \
                         columns first, then the aggregates)",
                    ));
                }
                if out
                    .iter()
                    .any(|o| matches!(o.item, Output::Column(c) if c == i))
                {
                    return Err(SqlError(format!(
                        "column {} appears twice in the SELECT list",
                        scope.column_at(i).name
                    )));
                }
                (Output::Column(i), scope.column_at(i).name.clone())
            }
            None => {
                let Expr::Function(call) = strip_parens(expr) else {
                    return Err(unsupported(&format!(
                        "the SELECT expression `{expr}` (v0 selects bare columns, COUNT(*) and SUM(column))"
                    )));
                };
                (Output::Agg(aggregate(scope, call)?), piece)
            }
        };
        let name = alias.map_or(default_name, |a| a.value.clone());
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
            return Err(SqlError(format!(
                "two result columns are named {name}; give one an alias"
            )));
        }
        out.push(Named { item, name });
    }
    Ok(out)
}

/// `COUNT(*)` or `SUM(<column>)`.
fn aggregate(scope: &Scope, call: &Function) -> Result<Agg, SqlError> {
    let Function {
        name,
        uses_odbc_syntax,
        parameters,
        args,
        within_group,
        filter,
        null_treatment,
        over,
    } = call;
    reject_if(over.is_some(), "a window function (OVER)")?;
    reject_if(filter.is_some(), "an aggregate FILTER clause")?;
    reject_if(
        *uses_odbc_syntax
            || !matches!(parameters, FunctionArguments::None)
            || !within_group.is_empty()
            || null_treatment.is_some(),
        "this function call syntax",
    )?;
    let func_name = &single_ident(name)?.value;
    let FunctionArguments::List(FunctionArgumentList {
        duplicate_treatment,
        args,
        clauses,
    }) = args
    else {
        return Err(unsupported(&format!("the call `{call}`")));
    };
    reject_if(
        duplicate_treatment.is_some(),
        "DISTINCT or ALL inside an aggregate",
    )?;
    reject_if(
        !clauses.is_empty(),
        "ORDER BY or other clauses inside an aggregate",
    )?;
    if func_name.eq_ignore_ascii_case("count") {
        match args.as_slice() {
            [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] => Ok(Agg {
                func: AggFn::Count,
                column: None,
            }),
            _ => Err(unsupported(&format!(
                "`{call}` (v0 supports COUNT(*) only; COUNT(column) skips NULLs)"
            ))),
        }
    } else if func_name.eq_ignore_ascii_case("sum") {
        match args.as_slice() {
            [FunctionArg::Unnamed(FunctionArgExpr::Expr(e))] => Ok(Agg {
                func: AggFn::Sum,
                column: Some(scope.require_column(e, "SUM over")?),
            }),
            _ => Err(unsupported(&format!("`{call}`"))),
        }
    } else {
        Err(unsupported(&format!(
            "the function `{func_name}` (v0's aggregates are COUNT(*) and SUM; MIN, MAX, AVG \
             and the rest are not)"
        )))
    }
}

/// GROUP BY: bare columns only, each once, and exactly the SELECT list's bare
/// columns. Returned in SELECT order, which is the output order.
fn group_by(
    scope: &Scope,
    clause: &GroupByExpr,
    outputs: &[Named],
) -> Result<Vec<usize>, SqlError> {
    let GroupByExpr::Expressions(exprs, modifiers) = clause else {
        return Err(unsupported("GROUP BY ALL"));
    };
    reject_if(
        !modifiers.is_empty(),
        "GROUP BY modifiers (ROLLUP, CUBE, …)",
    )?;
    let mut keys = Vec::new();
    for e in exprs {
        let i = scope
            .require_column(e, "the GROUP BY term")
            .map_err(|err| {
                // Final review Minor 4: SQLite accepts a GROUP BY term that
                // names a SELECT alias; v0 does not (the alias may not even be
                // a column), so say so rather than leave it looking like a
                // typo, when that is why the lookup failed.
                match strip_parens(e) {
                    Expr::Identifier(name)
                        if outputs
                            .iter()
                            .any(|o| o.name.eq_ignore_ascii_case(&name.value)) =>
                    {
                        SqlError(format!(
                            "{err} (GROUP BY must name the column, not the alias)"
                        ))
                    }
                    _ => err,
                }
            })?;
        if keys.contains(&i) {
            return Err(SqlError(format!(
                "column {} appears twice in GROUP BY",
                scope.column_at(i).name
            )));
        }
        keys.push(i);
    }
    let selected: Vec<usize> = outputs
        .iter()
        .filter_map(|o| match o.item {
            Output::Column(i) => Some(i),
            Output::Agg(_) => None,
        })
        .collect();
    if let Some(&i) = keys.iter().find(|i| !selected.contains(i)) {
        return Err(SqlError(format!(
            "column {} is in GROUP BY but not in the SELECT list; v0 needs every group key in \
             the view's output, or two groups could produce the same row (spec §5.2)",
            scope.column_at(i).name
        )));
    }
    if let Some(&i) = selected.iter().find(|i| !keys.contains(i)) {
        return Err(SqlError(format!(
            "column {} is in the SELECT list but neither in GROUP BY nor aggregated",
            scope.column_at(i).name
        )));
    }
    Ok(selected)
}

/// WHERE: one comparison between a column and a literal, or IS [NOT] NULL
/// (spec §6.1).
fn predicate(scope: &Scope, expr: &Expr) -> Result<Predicate, SqlError> {
    let not_supported = || {
        unsupported(&format!(
            "the WHERE clause `{expr}` (a v0 view filters on one comparison between a column \
             and a literal, or on IS [NOT] NULL)"
        ))
    };
    match strip_parens(expr) {
        Expr::IsNull(e) => Ok(Predicate::IsNull {
            column: scope.require_column(e, "IS NULL over")?,
        }),
        Expr::IsNotNull(e) => Ok(Predicate::IsNotNull {
            column: scope.require_column(e, "IS NOT NULL over")?,
        }),
        Expr::BinaryOp {
            op: BinaryOperator::And,
            ..
        } => Err(unsupported(
            "AND in WHERE (a v0 view has at most one predicate)",
        )),
        Expr::BinaryOp {
            op: BinaryOperator::Or,
            ..
        } => Err(unsupported("OR in WHERE")),
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            ..
        } => Err(unsupported("NOT in WHERE")),
        Expr::BinaryOp { left, op, right } => {
            let op = cmp_op(op).ok_or_else(not_supported)?;
            match (scope.column(left)?, scope.column(right)?) {
                (Some(column), None) => Ok(Predicate::Compare {
                    column,
                    op,
                    value: literal(right)?,
                }),
                (None, Some(column)) => Ok(Predicate::Compare {
                    column,
                    op: flip(op),
                    value: literal(left)?,
                }),
                (Some(_), Some(_)) => Err(unsupported(
                    "a comparison between two columns (v0 compares a column with a literal)",
                )),
                (None, None) => Err(not_supported()),
            }
        }
        _ => Err(not_supported()),
    }
}

fn cmp_op(op: &BinaryOperator) -> Option<CmpOp> {
    match op {
        BinaryOperator::Gt => Some(CmpOp::Gt),
        BinaryOperator::GtEq => Some(CmpOp::Ge),
        BinaryOperator::Lt => Some(CmpOp::Lt),
        BinaryOperator::LtEq => Some(CmpOp::Le),
        BinaryOperator::Eq => Some(CmpOp::Eq),
        BinaryOperator::NotEq => Some(CmpOp::Ne),
        _ => None,
    }
}

/// The operator that keeps a comparison's meaning when its operands swap
/// sides: `3 < v` is `v > 3`.
fn flip(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Ge => CmpOp::Le,
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Eq => CmpOp::Eq,
        CmpOp::Ne => CmpOp::Ne,
    }
}

/// `1L`: sqlparser accepts a long suffix that SQLite rejects as an
/// unrecognized token (measured, 3.53).
fn long_literal(text: &str) -> SqlError {
    unsupported(&format!(
        "the literal {text}L (SQLite does not accept an L suffix)"
    ))
}

/// An integer, a single-quoted string, or NULL (which `lower` rejects with a
/// pointer to IS NULL).
fn literal(expr: &Expr) -> Result<Value, SqlError> {
    let integer = |text: &str| {
        text.parse::<i64>().map(Value::Int).map_err(|_| {
            unsupported(&format!(
                "the literal {text} (v0's numeric literals are 64-bit integers)"
            ))
        })
    };
    match strip_parens(expr) {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(text, false) => integer(text),
            SqlValue::Number(text, true) => Err(long_literal(text)),
            SqlValue::SingleQuotedString(s) => Ok(Value::Text(s.clone())),
            SqlValue::Null => Ok(Value::Null),
            _ => Err(unsupported(&format!("the literal `{expr}`"))),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => match strip_parens(inner) {
            Expr::Value(v) => match &v.value {
                SqlValue::Number(text, false) => integer(&format!("-{text}")),
                SqlValue::Number(text, true) => Err(long_literal(text)),
                _ => Err(unsupported(&format!("the literal `{expr}`"))),
            },
            _ => Err(unsupported(&format!("the operand `{expr}`"))),
        },
        _ => Err(unsupported(&format!(
            "the operand `{expr}` (a comparison's other side must be a literal)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ivmlite_core::{lower_query, Database, Join, ViewQuery};

    /// `orders(region TEXT, amount INTEGER NOT NULL)` and
    /// `regions(name TEXT, manager TEXT)`.
    fn db() -> Database {
        let col = |name: &str, ty, nullable| Column {
            name: name.into(),
            ty,
            nullable,
        };
        Database::new(vec![
            Schema {
                table: "orders".into(),
                columns: vec![
                    col("region", ColumnType::Text, true),
                    col("amount", ColumnType::Integer, false),
                ],
            },
            Schema {
                table: "regions".into(),
                columns: vec![
                    col("name", ColumnType::Text, true),
                    col("manager", ColumnType::Text, true),
                ],
            },
        ])
    }

    fn ok(sql: &str) -> CompiledView {
        compile(sql, &db()).unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    fn err(sql: &str) -> String {
        match compile(sql, &db()) {
            Ok(v) => panic!("{sql} must be rejected, compiled to {v:?}"),
            Err(e) => e.0,
        }
    }

    fn plan_of(q: ViewQuery) -> Plan {
        lower_query(&q, &db()).expect("the expected query is legal")
    }

    fn count() -> Agg {
        Agg {
            func: AggFn::Count,
            column: None,
        }
    }

    fn sum(column: usize) -> Agg {
        Agg {
            func: AggFn::Sum,
            column: Some(column),
        }
    }

    fn query(group_by: Vec<usize>, aggs: Vec<Agg>, predicate: Predicate) -> ViewQuery {
        ViewQuery {
            group_by,
            aggs,
            predicate,
            join: None,
        }
    }

    #[test]
    fn compiles_the_spec_example() {
        // Spec §8.3's example view.
        let v = ok("SELECT region, SUM(amount), COUNT(*) FROM orders GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(vec![0], vec![sum(1), count()], Predicate::None))
        );
        assert_eq!(v.tables, vec!["orders".to_string()]);
        let names: Vec<&str> = v.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["region", "SUM(amount)", "COUNT(*)"]);
        let types: Vec<(ColumnType, bool)> = v.columns.iter().map(|c| (c.ty, c.nullable)).collect();
        assert_eq!(
            types,
            vec![
                (ColumnType::Text, true),
                // SUM over only NULLs is NULL (spec §6.1); COUNT(*) never is.
                (ColumnType::Integer, true),
                (ColumnType::Integer, false),
            ]
        );
    }

    #[test]
    fn names_resolve_case_insensitively_and_keep_their_declared_spelling() {
        let v = ok(r#"SELECT "REGION", count(*) FROM Orders GROUP BY Region"#);
        assert_eq!(
            v.plan,
            plan_of(query(vec![0], vec![count()], Predicate::None))
        );
        assert_eq!(v.tables, vec!["orders".to_string()]);
        assert_eq!(v.columns[0].name, "region");
    }

    #[test]
    fn select_all_is_accepted_as_a_plain_select() {
        // Final review Minor 3: SQLite treats `SELECT ALL` as a plain
        // SELECT, not a variant of DISTINCT.
        let v = ok("SELECT ALL region, COUNT(*) FROM orders GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(vec![0], vec![count()], Predicate::None))
        );
    }

    #[test]
    fn an_alias_names_the_result_column() {
        let v = ok("SELECT region AS r, SUM(amount) total FROM orders GROUP BY region");
        let names: Vec<&str> = v.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["r", "total"]);
    }

    #[test]
    fn the_select_order_of_group_keys_is_the_output_order() {
        // GROUP BY's order does not matter; the SELECT list's does.
        let v = ok("SELECT amount, region, COUNT(*) FROM orders GROUP BY region, amount");
        assert_eq!(
            v.plan,
            plan_of(query(vec![1, 0], vec![count()], Predicate::None))
        );
    }

    #[test]
    fn every_comparison_operator_and_both_literal_types_compile() {
        let cases = [
            (">", CmpOp::Gt),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            ("<=", CmpOp::Le),
            ("=", CmpOp::Eq),
            ("!=", CmpOp::Ne),
            ("<>", CmpOp::Ne),
        ];
        for (symbol, op) in cases {
            let sql = format!(
                "SELECT region, COUNT(*) FROM orders WHERE amount {symbol} 3 GROUP BY region"
            );
            let expected = Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(3),
            };
            assert_eq!(
                ok(&sql).plan,
                plan_of(query(vec![0], vec![count()], expected)),
                "{sql}"
            );
        }
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE region = 'it''s' GROUP BY region");
        let expected = Predicate::Compare {
            column: 0,
            op: CmpOp::Eq,
            value: Value::Text("it's".into()),
        };
        assert_eq!(v.plan, plan_of(query(vec![0], vec![count()], expected)));
    }

    #[test]
    fn a_literal_on_the_left_flips_the_operator() {
        // `3 < amount` is `amount > 3` (M1b Phase 2a, Ruling 1).
        for (sql_op, op) in [
            ("<", CmpOp::Gt),
            ("<=", CmpOp::Ge),
            (">", CmpOp::Lt),
            (">=", CmpOp::Le),
            ("=", CmpOp::Eq),
            ("!=", CmpOp::Ne),
        ] {
            let sql = format!(
                "SELECT region, COUNT(*) FROM orders WHERE 3 {sql_op} amount GROUP BY region"
            );
            let expected = Predicate::Compare {
                column: 1,
                op,
                value: Value::Int(3),
            };
            assert_eq!(
                ok(&sql).plan,
                plan_of(query(vec![0], vec![count()], expected)),
                "{sql}"
            );
        }
    }

    #[test]
    fn negative_literals_reach_i64_min() {
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE amount > -9223372036854775808 GROUP BY region");
        let expected = Predicate::Compare {
            column: 1,
            op: CmpOp::Gt,
            value: Value::Int(i64::MIN),
        };
        assert_eq!(v.plan, plan_of(query(vec![0], vec![count()], expected)));
    }

    #[test]
    fn is_null_parentheses_and_is_not_null_compile() {
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE (region IS NULL) GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(
                vec![0],
                vec![count()],
                Predicate::IsNull { column: 0 }
            ))
        );
        let v = ok("SELECT region, COUNT(*) FROM orders WHERE region IS NOT NULL GROUP BY region");
        assert_eq!(
            v.plan,
            plan_of(query(
                vec![0],
                vec![count()],
                Predicate::IsNotNull { column: 0 }
            ))
        );
    }

    #[test]
    fn a_join_with_aliases_and_a_reversed_on_compiles() {
        // The ON condition names the right table first, and its two key
        // columns sit at different positions (orders.region is column 0,
        // regions.manager column 1), so reading one side's index for the
        // other cannot go unnoticed.
        let v = ok(
            "SELECT r.name, SUM(o.amount) FROM orders AS o INNER JOIN regions r \
             ON r.manager = o.region GROUP BY r.name",
        );
        let expected = ViewQuery {
            join: Some(Join {
                right: "regions".into(),
                left_column: 0,
                right_column: 1,
            }),
            ..query(vec![2], vec![sum(1)], Predicate::None)
        };
        assert_eq!(v.plan, plan_of(expected));
        assert_eq!(v.tables, vec!["orders".to_string(), "regions".to_string()]);
    }

    #[test]
    fn an_unqualified_column_resolves_when_only_one_table_has_it() {
        let v = ok(
            "SELECT manager, COUNT(*) FROM orders JOIN regions ON region = name GROUP BY manager",
        );
        let expected = ViewQuery {
            join: Some(Join {
                right: "regions".into(),
                left_column: 0,
                right_column: 0,
            }),
            ..query(vec![3], vec![count()], Predicate::None)
        };
        assert_eq!(v.plan, plan_of(expected));
    }

    #[test]
    fn the_anchor_need_not_be_the_databases_first_table() {
        // Final review Minor 5: `db()` lists `orders` first and `regions`
        // second; the FROM clause names the anchor, not the database.
        let v = ok("SELECT regions.name, COUNT(*) FROM regions JOIN orders \
             ON regions.name = orders.region GROUP BY regions.name");
        let d = db();
        let expected = lower(&ResolvedView {
            anchor: &d.tables()[1],
            join: Some(ResolvedJoin {
                right: &d.tables()[0],
                left_column: 0,
                right_column: 0,
            }),
            group_by: vec![0],
            aggs: vec![count()],
            predicate: Predicate::None,
        })
        .expect("a legal view");
        assert_eq!(v.plan, expected);
        assert_eq!(v.tables, vec!["regions".to_string(), "orders".to_string()]);
    }

    #[test]
    fn lower_s_checks_apply_to_sql() {
        // `lower` holds the legality rules; the front end only resolves names.
        assert!(err("SELECT SUM(amount) FROM orders").contains("GROUP BY"));
        assert!(err("SELECT region FROM orders GROUP BY region").contains("aggs"));
        assert!(err("SELECT region, SUM(region) FROM orders GROUP BY region").contains("SUM"));
        assert!(
            err("SELECT region, COUNT(*) FROM orders WHERE amount > 'x' GROUP BY region")
                .contains("comparison")
        );
        assert!(
            err("SELECT region, COUNT(*) FROM orders WHERE amount = NULL GROUP BY region")
                .contains("IS NULL")
        );
        assert!(err(
            "SELECT a.region, COUNT(*) FROM orders a JOIN orders b ON a.region = b.region \
             GROUP BY a.region"
        )
        .contains("self-join"));
        assert!(err(
            "SELECT region, COUNT(*) FROM orders JOIN regions ON amount = name GROUP BY region"
        )
        .contains("same type"));
    }

    /// Spec §12.5: everything outside the subset is a hard error that names
    /// what it rejected.
    #[test]
    fn everything_outside_the_subset_is_rejected_by_name() {
        let cases = [
            ("SELECT region, COUNT(*) FROM orders GROUP BY region; SELECT 1", "exactly one SELECT"),
            ("INSERT INTO orders VALUES ('a', 1)", "exactly one SELECT"),
            ("WITH x AS (SELECT 1) SELECT region, COUNT(*) FROM orders GROUP BY region", "WITH"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region ORDER BY region", "ORDER BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region LIMIT 1", "LIMIT"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region UNION SELECT region, COUNT(*) FROM orders GROUP BY region", "compound"),
            ("SELECT DISTINCT region, COUNT(*) FROM orders GROUP BY region", "DISTINCT"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region HAVING COUNT(*) > 1", "HAVING"),
            ("SELECT COUNT(*)", "without FROM"),
            ("SELECT region, COUNT(*) FROM orders, regions GROUP BY region", "comma-separated"),
            ("SELECT region, COUNT(*) FROM (SELECT * FROM orders) GROUP BY region", "subquery"),
            ("SELECT region, COUNT(*) FROM main.orders GROUP BY region", "schema prefix"),
            ("SELECT region, COUNT(*) FROM nope GROUP BY region", "no such table"),
            ("SELECT region, COUNT(*) FROM orders LEFT JOIN regions ON region = name GROUP BY region", "non-inner join"),
            ("SELECT region, COUNT(*) FROM orders LEFT OUTER JOIN regions ON region = name GROUP BY region", "non-inner join"),
            ("SELECT region, COUNT(*) FROM orders GLOBAL JOIN regions ON region = name GROUP BY region", "GLOBAL JOIN"),
            ("SELECT region, COUNT(*) FROM orders CROSS JOIN regions GROUP BY region", "non-inner join"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions USING (region) GROUP BY region", "without ON"),
            ("SELECT region, COUNT(*) FROM orders NATURAL JOIN regions GROUP BY region", "join"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name AND manager = 'x' GROUP BY region", "join condition"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = 'x' GROUP BY region", "join condition"),
            ("SELECT region, COUNT(*) FROM orders o JOIN regions o ON o.region = o.name GROUP BY region", "both tables"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name JOIN regions r2 ON region = r2.name GROUP BY region", "more than two tables"),
            ("SELECT * FROM orders GROUP BY region", "SELECT *"),
            ("SELECT region, amount + 1, COUNT(*) FROM orders GROUP BY region", "SELECT expression"),
            ("SELECT region, COUNT(amount) FROM orders GROUP BY region", "COUNT(*) only"),
            ("SELECT region, COUNT(DISTINCT amount) FROM orders GROUP BY region", "DISTINCT"),
            ("SELECT region, SUM(amount + 1) FROM orders GROUP BY region", "SUM over"),
            ("SELECT region, MIN(amount) FROM orders GROUP BY region", "MIN"),
            ("SELECT region, AVG(amount) FROM orders GROUP BY region", "AVG"),
            ("SELECT region, SUM(amount) OVER () FROM orders GROUP BY region", "window"),
            ("SELECT COUNT(*), region FROM orders GROUP BY region", "after an aggregate"),
            ("SELECT region, region, COUNT(*) FROM orders GROUP BY region", "twice in the SELECT list"),
            ("SELECT region, COUNT(*) AS region FROM orders GROUP BY region", "named region"),
            ("SELECT region, amount, COUNT(*) FROM orders GROUP BY region", "neither in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region, amount", "not in the SELECT list"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region, region", "twice in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY 1", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY lower(region)", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1 AND amount < 5 GROUP BY region", "at most one predicate"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1 OR amount < 5 GROUP BY region", "OR in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE NOT amount > 1 GROUP BY region", "NOT in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE region LIKE 'a%' GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount IN (1, 2) GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount BETWEEN 1 AND 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > (SELECT 1) GROUP BY region", "operand"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > amount GROUP BY region", "two columns"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1.5 GROUP BY region", "64-bit integers"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1L GROUP BY region", "L suffix"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > -1L GROUP BY region", "L suffix"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 9223372036854775808 GROUP BY region", "64-bit integers"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount + 1 > 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE nope > 1 GROUP BY region", "no such column"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name WHERE x.region > 'a' GROUP BY region", "no such table"),
            ("SELECT region, COUNT(*) FROM orders o JOIN regions r ON o.region = r.name GROUP BY o.region, r.name", "not in the SELECT list"),
            ("SELECT k, COUNT(*) FROM orders JOIN regions ON region = name GROUP BY k", "no such column"),
        ];
        for (sql, expected) in cases {
            let e = err(sql);
            assert!(
                e.contains(expected),
                "{sql}\n  error: {e}\n  expected it to mention: {expected}"
            );
        }
    }

    /// Final review Minor 4: two error hints. SQLite's double-quoted-string
    /// fallback and GROUP BY-by-alias are both accepted by SQLite but not by
    /// v0; the rejection should say why, not just "no such column".
    #[test]
    fn a_double_quoted_unresolved_identifier_hints_at_single_quotes() {
        let e = err(r#"SELECT region, COUNT(*) FROM orders WHERE region = "zz" GROUP BY region"#);
        assert!(e.contains("no such column: zz"), "{e}");
        assert!(e.contains("single quote"), "{e}");
    }

    #[test]
    fn group_by_naming_a_select_alias_hints_at_the_column() {
        let e = err("SELECT region AS r, COUNT(*) FROM orders GROUP BY r");
        assert!(e.contains("no such column: r"), "{e}");
        assert!(e.contains("alias"), "{e}");
    }

    #[test]
    fn an_ambiguous_unqualified_column_is_rejected() {
        let db = Database::new(vec![
            db().tables()[0].clone(),
            Schema {
                table: "others".into(),
                columns: db().tables()[0].columns.clone(),
            },
        ]);
        let e = compile(
            "SELECT region, COUNT(*) FROM orders JOIN others ON orders.region = others.region GROUP BY region",
            &db,
        )
        .expect_err("region is in both tables");
        assert!(e.0.contains("ambiguous"), "{}", e.0);
    }

    #[test]
    fn a_parse_error_is_reported_as_such() {
        assert!(err("SELEC region FROM orders").contains("cannot parse"));
    }
}
