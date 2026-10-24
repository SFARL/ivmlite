use ivmlite_core::{
    lower, lower_source, Agg, AggFn, BinaryOp, CmpOp, Column, ColumnType, Plan, PlanError,
    Predicate, ResolvedJoin, ResolvedView, ScalarExpr, Schema, UnaryOp, Value,
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
    /// The view's output columns in actual SELECT order, independently of
    /// internal group keys and aggregate argument slots.
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
/// come before the aggregates, an aggregate is `COUNT(*)`, `COUNT(<column>)`,
/// or `SUM`, `TOTAL`, `AVG`, `MIN` or `MAX` of a column (M2c spec §4),
/// `<op>` is one of `>` `>=` `<` `<=` `=` `!=` `<>`, and a literal is a
/// number, a single-quoted string or `X'…'`. Anything else is an error.
fn compile_legacy(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError> {
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| SqlError(format!("cannot parse the view's SQL: {e}")))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(unsupported("anything but exactly one SELECT statement"));
    };
    let select = plain_select(query)?;
    let (anchor, join) = from_clause(select, catalog)?;
    let scope = Scope::new(&anchor, join.as_ref().map(|j| &j.table), catalog);
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

/// Preserve the canonical encoding of every formerly accepted SQL definition.
pub fn compile(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError> {
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| SqlError(format!("cannot parse the view's SQL: {e}")))?;
    if let [Statement::Query(query)] = statements.as_slice() {
        if let SetExpr::Select(select) = query.body.as_ref() {
            for item in &select.projection {
                if let SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } = item
                {
                    check_depth(e, 0)?;
                }
            }
            if let Some(e) = &select.selection {
                check_depth(e, 0)?;
            }
            if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
                for e in exprs {
                    check_depth(e, 0)?;
                }
            }
            for source in &select.from {
                for join in &source.joins {
                    if let JoinOperator::Join(JoinConstraint::On(e))
                    | JoinOperator::Inner(JoinConstraint::On(e)) = &join.join_operator
                    {
                        check_depth(e, 0)?;
                    }
                }
            }
        }
    }
    if let Ok(view) = compile_legacy(sql, catalog) {
        return Ok(view);
    }
    compile_shapes(sql, catalog)
}

fn check_depth(expr: &Expr, depth: usize) -> Result<(), SqlError> {
    if depth > ivmlite_core::MAX_EXPRESSION_DEPTH {
        return Err(SqlError(
            "expression or conjunction exceeds depth limit 64".into(),
        ));
    }
    match expr {
        Expr::Nested(e) | Expr::UnaryOp { expr: e, .. } | Expr::IsNull(e) | Expr::IsNotNull(e) => {
            check_depth(e, depth + 1)?
        }
        Expr::BinaryOp { left, right, .. } => {
            check_depth(left, depth + 1)?;
            check_depth(right, depth + 1)?;
        }
        Expr::Function(call) => {
            if let FunctionArguments::List(args) = &call.args {
                for arg in &args.args {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) = arg {
                        check_depth(e, depth + 1)?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn compile_shapes(sql: &str, catalog: &dyn Catalog) -> Result<CompiledView, SqlError> {
    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| SqlError(format!("cannot parse the view's SQL: {e}")))?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return Err(unsupported("anything but exactly one SELECT statement"));
    };
    let select = plain_select(query)?;
    let (anchor, joining) = from_clause(select, catalog)?;
    let scope = Scope::new(&anchor, joining.as_ref().map(|j| &j.table), catalog);
    let join = joining.as_ref().map(|j| join_on(&scope, j)).transpose()?;
    let keys = shape_group_by(&scope, &select.group_by, &select.projection)?;
    let mut expressions = Vec::new();
    let mut aggs = Vec::new();
    let mut arguments: Vec<ScalarExpr> = keys.iter().copied().map(ScalarExpr::Column).collect();
    let pieces = select_list_pieces(sql, select.projection.len())?;
    let mut columns: Vec<Column> = Vec::new();
    for (item, piece) in select.projection.iter().zip(pieces) {
        let mut push = |expr: ScalarExpr,
                        name: String,
                        ty: ColumnType,
                        nullable: bool|
         -> Result<(), SqlError> {
            if name.eq_ignore_ascii_case("__ivm_identity") {
                return Err(SqlError(
                    "a result column is named __ivm_identity, which ivmlite reserves".into(),
                ));
            }
            if columns.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                return Err(SqlError(format!(
                    "two result columns are named {name}; give one an alias"
                )));
            }
            columns.push(Column { name, ty, nullable });
            expressions.push(expr);
            Ok(())
        };
        let (expr, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias)),
            SelectItem::Wildcard(options) | SelectItem::QualifiedWildcard(_, options) => {
                if !keys.is_empty() {
                    return Err(unsupported("SELECT * in a grouped query"));
                }
                if options != &Default::default() {
                    return Err(unsupported("wildcard options"));
                }
                let qualifier = match item {
                    SelectItem::QualifiedWildcard(
                        sqlparser::ast::SelectItemQualifiedWildcardKind::ObjectName(name),
                        _,
                    ) => Some(single_ident(name)?.value.as_str()),
                    SelectItem::Wildcard(_) => None,
                    _ => return Err(unsupported("an expression-qualified wildcard")),
                };
                let mut found = false;
                for (source, offset) in scope.sources() {
                    if qualifier.is_none_or(|q| q.eq_ignore_ascii_case(&source.qualifier)) {
                        found = true;
                        for (i, c) in source.schema.columns.iter().enumerate() {
                            push(
                                ScalarExpr::Column(offset + i),
                                c.name.clone(),
                                c.ty,
                                c.nullable,
                            )?;
                        }
                    }
                }
                if !found {
                    return Err(SqlError(format!(
                        "no such table: {}",
                        qualifier.unwrap_or_default()
                    )));
                }
                continue;
            }
            _ => return Err(unsupported("multi-name aliases")),
        };
        let resolved = shape_expr(
            &scope,
            expr,
            if keys.is_empty() { None } else { Some(&keys) },
            &mut aggs,
            &mut arguments,
            0,
        )?;
        let (ty, nullable) = match &resolved {
            ScalarExpr::Column(c) if keys.is_empty() => {
                let col = scope.column_at(*c);
                (col.ty, col.nullable)
            }
            ScalarExpr::Column(c) if *c < keys.len() => {
                let col = scope.column_at(keys[*c]);
                (col.ty, col.nullable)
            }
            ScalarExpr::Column(c) => {
                let a = &aggs[*c - keys.len()];
                (
                    match a.func {
                        AggFn::Count => ColumnType::Integer,
                        AggFn::Sum => ColumnType::Numeric,
                        AggFn::Total | AggFn::Avg => ColumnType::Real,
                        AggFn::Min | AggFn::Max => {
                            match &arguments[a.column.expect("extremum argument")] {
                                ScalarExpr::Column(i) => scope.column_at(*i).ty,
                                _ => ColumnType::Numeric,
                            }
                        }
                    },
                    a.func != AggFn::Count,
                )
            }
            _ => (ColumnType::Numeric, true),
        };
        let name = alias
            .map(|a| a.value.clone())
            .unwrap_or_else(|| match scope.column(expr) {
                Ok(Some(c)) => scope.column_at(c).name.clone(),
                _ => piece,
            });
        push(resolved, name, ty, nullable)?;
    }
    if keys.is_empty() && !aggs.is_empty() {
        return Err(unsupported("global aggregates without GROUP BY"));
    }
    if !keys.is_empty() && aggs.is_empty() {
        return Err(unsupported("GROUP BY with no aggs"));
    }
    let predicate = select
        .selection
        .as_ref()
        .map(|e| predicate(&scope, e))
        .transpose()?
        .unwrap_or(Predicate::None);
    let mut plan = lower_source(&anchor.schema, join.as_ref(), &predicate)?;
    if keys.is_empty() {
        plan = Plan::Map {
            input: Box::new(plan),
            expressions,
        };
    } else {
        let arity = scope.sources().map(|(s, _)| s.schema.arity()).sum();
        for expr in &arguments {
            expr.validate(arity).map_err(|e| SqlError(e.0))?;
        }
        plan = Plan::Map {
            input: Box::new(plan),
            expressions: arguments,
        };
        plan = Plan::AggregateCoherent {
            input: Box::new(plan),
            group_by: (0..keys.len()).collect(),
            aggs,
        };
        plan = Plan::Map {
            input: Box::new(plan),
            expressions,
        };
    }
    let mut tables = vec![anchor.schema.table.clone()];
    tables.extend(scope.right.map(|r| r.schema.table.clone()));
    plan.validate_projections().map_err(|e| SqlError(e.0))?;
    Ok(CompiledView {
        plan,
        columns,
        tables,
    })
}

fn shape_group_by(
    scope: &Scope,
    clause: &GroupByExpr,
    outputs: &[SelectItem],
) -> Result<Vec<usize>, SqlError> {
    let GroupByExpr::Expressions(exprs, modifiers) = clause else {
        return Err(unsupported("GROUP BY ALL"));
    };
    reject_if(
        !modifiers.is_empty(),
        "GROUP BY modifiers (ROLLUP, CUBE, …)",
    )?;
    let mut keys = Vec::new();
    for expr in exprs {
        let c=scope.require_column(expr,"the GROUP BY term").map_err(|e| {
            if outputs.iter().any(|o|matches!(o,SelectItem::ExprWithAlias{alias,..} if expr.to_string().eq_ignore_ascii_case(&alias.value))) {SqlError(format!("{e} (GROUP BY must name the column, not the alias)"))} else {e}
        })?;
        if keys.contains(&c) {
            return Err(SqlError(format!(
                "column {} appears twice in GROUP BY",
                scope.column_at(c).name
            )));
        }
        keys.push(c);
    }
    Ok(keys)
}

fn shape_expr(
    scope: &Scope,
    expr: &Expr,
    keys: Option<&[usize]>,
    aggs: &mut Vec<Agg>,
    arguments: &mut Vec<ScalarExpr>,
    depth: usize,
) -> Result<ScalarExpr, SqlError> {
    if depth > ivmlite_core::MAX_EXPRESSION_DEPTH {
        return Err(SqlError("scalar expression exceeds depth limit 64".into()));
    }
    if let Some(c) = scope.column(expr)? {
        return Ok(ScalarExpr::Column(match keys {
            None => c,
            Some(keys) => keys.iter().position(|k| *k == c).ok_or_else(|| {
                SqlError(format!(
                    "column {} is in the SELECT list but neither in GROUP BY nor aggregated",
                    scope.column_at(c).name
                ))
            })?,
        }));
    }
    let recurse = |e, aggs: &mut Vec<Agg>, arguments: &mut Vec<ScalarExpr>| {
        shape_expr(scope, e, keys, aggs, arguments, depth + 1)
    };
    match expr {
        Expr::Nested(e) => recurse(e, aggs, arguments),
        Expr::Value(_) => Ok(ScalarExpr::Literal(literal(scope, expr)?)),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } if matches!(strip_parens(inner),Expr::Value(v) if matches!(v.value,SqlValue::Number(_, _))) => {
            Ok(ScalarExpr::Literal(literal(scope, expr)?))
        }
        Expr::UnaryOp { op, expr } => Ok(ScalarExpr::Unary {
            op: match op {
                UnaryOperator::Plus => UnaryOp::Plus,
                UnaryOperator::Minus => UnaryOp::Minus,
                _ => return Err(unsupported("this scalar unary operator")),
            },
            expr: Box::new(recurse(expr, aggs, arguments)?),
        }),
        Expr::BinaryOp { left, op, right } => Ok(ScalarExpr::Binary {
            op: match op {
                BinaryOperator::Plus => BinaryOp::Add,
                BinaryOperator::Minus => BinaryOp::Subtract,
                BinaryOperator::Multiply => BinaryOp::Multiply,
                BinaryOperator::Divide => BinaryOp::Divide,
                BinaryOperator::Modulo => BinaryOp::Remainder,
                _ => return Err(unsupported("this scalar binary operator")),
            },
            left: Box::new(recurse(left, aggs, arguments)?),
            right: Box::new(recurse(right, aggs, arguments)?),
        }),
        Expr::Function(call) => {
            let (func, arg) = aggregate_parts(call)?;
            if keys.is_none() {
                return Err(unsupported("nested or global aggregates without GROUP BY"));
            }
            let column = match arg {
                None => None,
                Some(arg) => {
                    let value = shape_expr(
                        scope,
                        arg,
                        None,
                        &mut Vec::new(),
                        &mut Vec::new(),
                        depth + 1,
                    )?;
                    Some(match arguments.iter().position(|x| *x == value) {
                        Some(i) => i,
                        None => {
                            let i = arguments.len();
                            arguments.push(value);
                            i
                        }
                    })
                }
            };
            let agg = Agg { func, column };
            let i = match aggs.iter().position(|a| *a == agg) {
                Some(i) => i,
                None => {
                    let i = aggs.len();
                    aggs.push(agg);
                    i
                }
            };
            Ok(ScalarExpr::Column(
                keys.expect("grouped aggregate").len() + i,
            ))
        }
        _ => Err(unsupported(&format!("the SELECT expression `{expr}`"))),
    }
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
    catalog: &'a dyn Catalog,
    anchor: &'a Source,
    right: Option<&'a Source>,
}

impl<'a> Scope<'a> {
    fn new(anchor: &'a Source, right: Option<&'a Source>, catalog: &'a dyn Catalog) -> Self {
        Scope {
            anchor,
            right,
            catalog,
        }
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
            // An aggregate's type is informational: what SQLite's function
            // returns (M2c spec §4). SUM is INTEGER or REAL, so NUMERIC; MIN
            // and MAX return one of the argument's values, so its affinity.
            Output::Agg(agg) => Column {
                name: self.name.clone(),
                ty: match (agg.func, agg.column) {
                    (AggFn::Count, _) => ColumnType::Integer,
                    (AggFn::Sum, _) => ColumnType::Numeric,
                    (AggFn::Total | AggFn::Avg, _) => ColumnType::Real,
                    (AggFn::Min | AggFn::Max, Some(c)) => scope.column_at(c).ty,
                    (AggFn::Min | AggFn::Max, None) => {
                        unreachable!("`aggregate` gives MIN and MAX a column")
                    }
                },
                // SUM, AVG, MIN and MAX over no non-NULL value are NULL (spec
                // §6.1, M2c spec §4), and SUM, TOTAL and AVG over both
                // infinities are NULL (M2c spec amendment 5); only COUNT is
                // never NULL.
                nullable: agg.func != AggFn::Count,
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
                        "the SELECT expression `{expr}` (ivmlite selects bare group-by columns and COUNT(*), COUNT(column), SUM, TOTAL, AVG, MIN and MAX of a column)"
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

/// `COUNT(*)`, or `COUNT`, `SUM`, `TOTAL` or `AVG` of a column.
fn aggregate(scope: &Scope, call: &Function) -> Result<Agg, SqlError> {
    let (func, arg) = aggregate_parts(call)?;
    Ok(Agg {
        func,
        column: arg
            .map(|e| scope.require_column(e, &format!("{} over", func.sql_name())))
            .transpose()?,
    })
}

fn aggregate_parts(call: &Function) -> Result<(AggFn, Option<&Expr>), SqlError> {
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
    let func = match func_name.to_ascii_lowercase().as_str() {
        "count" => AggFn::Count,
        "sum" => AggFn::Sum,
        "total" => AggFn::Total,
        "avg" => AggFn::Avg,
        "min" => AggFn::Min,
        "max" => AggFn::Max,
        _ => {
            return Err(unsupported(&format!(
            "the function `{func_name}` (ivmlite's aggregates are COUNT, SUM, TOTAL, AVG, MIN and MAX)"
        )))
        }
    };
    match (func, args.as_slice()) {
        (AggFn::Count, [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)]) => Ok((func, None)),
        (_, [FunctionArg::Unnamed(FunctionArgExpr::Expr(e))]) => Ok((func, Some(e))),
        _ => Err(unsupported(&format!("`{call}`"))),
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
    predicate_at(scope, expr, 0)
}
fn predicate_at(scope: &Scope, expr: &Expr, depth: usize) -> Result<Predicate, SqlError> {
    if depth > ivmlite_core::MAX_EXPRESSION_DEPTH {
        return Err(SqlError("WHERE conjunction exceeds depth limit 64".into()));
    }
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
            left,
            right,
        } => Ok(Predicate::And(
            Box::new(predicate_at(scope, left, depth + 1)?),
            Box::new(predicate_at(scope, right, depth + 1)?),
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
                    value: literal(scope, right)?,
                }),
                (None, Some(column)) => Ok(Predicate::Compare {
                    column,
                    op: flip(op),
                    value: literal(scope, left)?,
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

/// A numeric token as SQLite reads it: INTEGER when it has no `.` or
/// exponent and fits `i64`, else REAL (M2c spec §5).
fn number(scope: &Scope, text: &str) -> Result<Value, SqlError> {
    let integral = !text.contains(['.', 'e', 'E']);
    if integral {
        if let Ok(n) = text.parse::<i64>() {
            return Ok(Value::Int(n));
        }
    }
    text.parse::<f64>()
        .map_err(|_| unsupported(&format!("the literal {text}")))?;
    scope
        .catalog
        .parse_real_literal(text)
        .map(Value::Real)
        .map_err(SqlError::from)
}

/// A BLOB literal `X'…'`: an even number of hex digits.
fn blob(hex: &str) -> Result<Value, SqlError> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(SqlError(format!(
            "the BLOB literal X'{hex}' is not an even number of hex digits"
        )));
    }
    Ok(Value::Blob(
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("two hex digits"))
            .collect(),
    ))
}

/// A number, a single-quoted string, a BLOB literal `X'…'`, or NULL (which
/// `lower` rejects with a pointer to IS NULL).
fn literal(scope: &Scope, expr: &Expr) -> Result<Value, SqlError> {
    match strip_parens(expr) {
        Expr::Value(v) => match &v.value {
            SqlValue::Number(text, false) => number(scope, text),
            SqlValue::Number(text, true) => Err(long_literal(text)),
            SqlValue::SingleQuotedString(s) => Ok(Value::Text(s.clone())),
            SqlValue::HexStringLiteral(hex) => blob(hex),
            SqlValue::Null => Ok(Value::Null),
            _ => Err(unsupported(&format!("the literal `{expr}`"))),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => match strip_parens(inner) {
            Expr::Value(v) => match &v.value {
                SqlValue::Number(text, false) => number(scope, &format!("-{text}")),
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
                // SUM is INTEGER or REAL (M2c spec §4), so its type is NUMERIC.
                (ColumnType::Numeric, true),
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
    fn unsupported_aggregate_diagnostic_lists_current_functions() {
        assert_eq!(
            err("SELECT region, group_concat(amount) FROM orders GROUP BY region"),
            "the function `group_concat` (ivmlite's aggregates are COUNT, SUM, TOTAL, AVG, MIN and MAX) is not supported in an ivmlite v0 view (spec §13)"
        );
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
    fn real_and_blob_literals_compile_as_sqlite_reads_them() {
        let lit = |sql: &str| match ok(sql).plan {
            Plan::Aggregate { input, .. } => match *input {
                Plan::Project { input, .. } => match *input {
                    Plan::Filter {
                        predicate: Predicate::Compare { value, .. },
                        ..
                    } => value,
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        let w = |cond: &str| {
            format!("SELECT region, COUNT(*) FROM orders WHERE {cond} GROUP BY region")
        };
        assert_eq!(lit(&w("amount > 1.5")), Value::Real(1.5));
        assert_eq!(lit(&w("amount > -2.5e1")), Value::Real(-25.0));
        assert_eq!(
            lit(&w("amount > 9223372036854775808")),
            Value::Real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(
            lit(&w("amount > -9223372036854775808")),
            Value::Int(i64::MIN)
        );
        assert_eq!(
            lit(&w("amount > '7'")),
            Value::Int(7),
            "numeric affinity for an INTEGER column"
        );
        assert_eq!(
            lit(&w("region = 7")),
            Value::Text("7".into()),
            "TEXT affinity for a TEXT column"
        );
        assert_eq!(lit(&w("region = X'00ff'")), Value::Blob(vec![0, 0xff]));
        assert!(err(&w("region = X'0'")).contains("hex"));
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
        ok("SELECT region, COUNT(*) FROM orders WHERE amount > 'x' GROUP BY region");
        assert!(
            err("SELECT region, COUNT(*) FROM orders WHERE region > 2.5 GROUP BY region")
                .contains("REAL literal cannot be compared with the TEXT-affinity column region")
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
        .contains("amount has INTEGER affinity and regions.name has TEXT affinity"));
    }

    #[test]
    fn the_new_aggregates_compile_with_sqlites_names_and_types() {
        let v = ok(
            "SELECT region, COUNT(amount), SUM(region), TOTAL(amount), avg(amount), MIN(amount), \
             max(region) FROM orders GROUP BY region",
        );
        let got: Vec<(String, ColumnType, bool)> = v
            .columns
            .iter()
            .map(|c| (c.name.clone(), c.ty, c.nullable))
            .collect();
        assert_eq!(
            got,
            vec![
                ("region".to_string(), ColumnType::Text, true),
                ("COUNT(amount)".to_string(), ColumnType::Integer, false),
                ("SUM(region)".to_string(), ColumnType::Numeric, true),
                ("TOTAL(amount)".to_string(), ColumnType::Real, true),
                ("avg(amount)".to_string(), ColumnType::Real, true),
                ("MIN(amount)".to_string(), ColumnType::Integer, true),
                ("max(region)".to_string(), ColumnType::Text, true),
            ]
        );
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
            ("SELECT region, amount + 1, COUNT(*) FROM orders GROUP BY region", "neither in GROUP BY"),
            ("SELECT region, COUNT(DISTINCT amount) FROM orders GROUP BY region", "DISTINCT"),
            ("SELECT region, SUM(amount) OVER () FROM orders GROUP BY region", "window"),
            ("SELECT region, region, COUNT(*) FROM orders GROUP BY region", "two result columns"),
            ("SELECT region, COUNT(*) AS region FROM orders GROUP BY region", "named region"),
            ("SELECT region, amount, COUNT(*) FROM orders GROUP BY region", "neither in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY region, region", "twice in GROUP BY"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY 1", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders GROUP BY lower(region)", "GROUP BY term"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1 OR amount < 5 GROUP BY region", "OR in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE NOT amount > 1 GROUP BY region", "NOT in WHERE"),
            ("SELECT region, COUNT(*) FROM orders WHERE region LIKE 'a%' GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount IN (1, 2) GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount BETWEEN 1 AND 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > (SELECT 1) GROUP BY region", "operand"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > amount GROUP BY region", "two columns"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > 1L GROUP BY region", "L suffix"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount > -1L GROUP BY region", "L suffix"),
            ("SELECT region, COUNT(*) FROM orders WHERE amount + 1 > 2 GROUP BY region", "WHERE clause"),
            ("SELECT region, COUNT(*) FROM orders WHERE nope > 1 GROUP BY region", "no such column"),
            ("SELECT region, COUNT(*) FROM orders JOIN regions ON region = name WHERE x.region > 'a' GROUP BY region", "no such table"),
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
