# Zero: concurrent view maintenance transactions

[libSQL issue 1631](https://github.com/tursodatabase/libsql/issues/1631), by
aboodman, requests support for SQLite's `begin concurrent` branch. The
[original body](extracted/issue-1631-body.md) describes an existing Zero design:

1. Maintain client query views on multiple threads.
2. For each incoming transaction, interleave writes and reads; correctness
   requires that order, so all writes cannot simply be applied first.
3. Roll back each worker's concurrent transaction after computing its output.
4. Use a single writer to persist the input writes at the end.

The report says eager write locks otherwise conflict. This is concrete evidence
of a transaction/concurrency requirement, but it is not a published SQL workload
or a claim that Zero needs ivmlite. See the [raw issue](issue-1631.json) and
[empty comment list](issue-1631-comments.json); the issue was open at capture.

No schema, client queries, mutation trace, expected outputs or benchmark dataset
is included in the report. Do not replace this architecture with a fictional
single-table demo and call it a reproduction. Phase support: not evaluated.
