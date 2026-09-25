# Datasette: facets, counts and suggestions

## Separate original cases

| Report | User problem or experiment | Snapshot and evidence |
|---|---|---|
| [1684](https://github.com/simonw/datasette/issues/1684) | A real deployment disabled facets because large tables were slow; wants selective disabling | [body](extracted/issue-1684-body.md), [comments](issue-1684-comments.json); open at capture |
| [1259](https://github.com/simonw/datasette/issues/1259) | Reuse a CTE across country and fuel facet counts | [body and original results](extracted/issue-1259-body.md), [SQL](extracted/issue-1259-body-block-1.sql), [comments](issue-1259-comments.json); open at capture |
| [1513](https://github.com/simonw/datasette/issues/1513) | Retrieve rows, total count and multiple facets with one CTE/UNION query | [body](extracted/issue-1513-body.md), [experiments and conclusion](issue-1513-comments.json); closed at capture |
| [1111](https://github.com/simonw/datasette/issues/1111) | Metadata request spends about 43 seconds on a 23 GB pageview database | [body, request response and count command](extracted/issue-1111-body.md), [comments](issue-1111-comments.json); open at capture |
| [859](https://github.com/simonw/datasette/issues/859) | Database page exceeds ten seconds for a roughly 600 MB scraper database | [body and workaround](extracted/issue-859-body.md), [comments](issue-859-comments.json); open at capture |
| [2406](https://github.com/simonw/datasette/issues/2406) | Facet suggestions accumulate per-column query timeouts | [body](extracted/issue-2406-body.md), [patch discussion](issue-2406-comments.json); closed at capture |

All six issue responses and comment lists are retained as raw JSON. Timings,
database sizes and row counts above are the reporters' measurements.

## Original query examples

The [SQLite forum thread](https://sqlite.org/forum/forumpost/c0e0fcbe36?hist=&t=c)
contains a particularly concrete five-query page for Kentucky county COVID data:

- [Rows](extracted/kentucky-rows.sql), ordered by date and limited to 101.
- [Total count](extracted/kentucky-count.sql).
- Facets for [state](extracted/kentucky-facet-state.sql),
  [county](extracted/kentucky-facet-county.sql) and
  [FIPS](extracted/kentucky-facet-fips.sql), retaining filters, NULL handling,
  grouping, tie ordering and limit 31.
- The author's [combined CTE/UNION ALL attempt](extracted/kentucky-combined.sql).

These are extracted from the first original post revision in the captured
history page. The [decoded post](extracted/forum-original-post.md) and
[complete HTML including revisions and replies](forum.html) retain its context.
No semicolons, semantic corrections or equivalent-query claims have been added.

Issue 1513 also provides the original
[power-plant query](extracted/issue-1513-comment-970738130-block-1.sql),
[COVID query](extracted/issue-1513-comment-970766486-block-1.sql),
[local reproduction commands](extracted/issue-1513-comment-970845844-block-1.sh.txt),
and [query trace](extracted/issue-1513-comment-970845844-block-2.json).
Other SQL and trace blocks are in `extracted/`, each with a source pointer in
the root extraction manifest. Full-text search motivates issue 1513, but the
archived SQL variants must be checked individually for the filters they use.

## Preserve the alternatives and negative results

The author [concluded](https://github.com/simonw/datasette/issues/1513#issuecomment-970855084)
that the giant CTE/UNION ALL experiment was slower overall. It is an example of
a measured query-reuse attempt, not proof that persistent incremental views
will win. Independent per-facet timeout behavior also matters to that page.

Issue 2406 explores limiting the rows inspected for *suggestions*, reports an
improvement, and is closed. Suggestions need not have the same completeness
contract as exact facet counts. Reports 1111 and 859 also discuss avoiding or
limiting expensive counts. Preserve those alternatives when evaluating demand.

## Data and reproduction gaps

The 2021 commands reference `https://covid-19.datasettes.com/covid.db`, reported
in the discussion as an 805.2 MB database. That database has not been downloaded,
and no exact historical snapshot, full DDL or write stream is included here.
The public Global Power Plants database page returned HTTP 404 during collection;
the failed request is recorded in the root manifest. The private scraper and
pageview databases are not provided by their issue reports. Metadata column
lists are not complete table definitions.

This collection preserves the SQL, commands, observed responses and source
locations without replacing missing databases with invented fixtures. None of
the archived commands or SQL has been executed. Phase support: not evaluated.
