# SQLite forum: generic incremental view maintenance

The January 2023 [forum discussion](https://sqlite.org/forum/forumpost/c7437e2f43)
asks whether incremental view maintenance exists for SQLite, referring to Noria
as an example. The requester wants to retain SQLite and avoid writing a
different trigger implementation for every query. The discussion includes
objections, trigger suggestions and computation/storage tradeoffs.

The [HTML snapshot](forum.html) preserves the discussion and its author/date
attribution. This is explicit demand for a capability, but no concrete schema,
query, dataset or update sequence is supplied. It cannot establish a workload's
frequency, performance benefit or willingness to adopt this project.

Keep it as requirement evidence alongside the cases with source code. No
synthetic benchmark has been substituted. Phase support: not evaluated.
