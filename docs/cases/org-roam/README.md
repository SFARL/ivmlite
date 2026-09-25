# Org-roam: assembling complete note records

## Original demand

[Org-roam issue 1997](https://github.com/org-roam/org-roam/issues/1997) proposes
denormalizing node information from nodes, aliases, citations, refs, tags and
links to make interactive note lookup and dependent applications faster.
The author reports tests on 9,554 notes. Maintainer discussion also asks about
write overhead and additional maintenance code. The issue was open when captured.

There are three related, distinct sources here:

1. The proposal and its comments: [issue body](extracted/issue-1997-body.md),
   [raw issue](issue-1997.json), [comments](issue-1997-comments.json).
2. [Vulpea PR 116](https://github.com/d12frosted/vulpea/pull/116): an application
   implementation with its own synchronization and benchmarks. Captured
   [PR metadata](vulpea-pr-116.json) and [all 13 file patches](vulpea-pr-116-files.json)
   preserve context; this is not the later SQL trigger implementation.
3. The 2024 [trigger experiment](https://org-roam.discourse.group/t/3483):
   [first 20 posts](forum-topic.json), [remaining eight](forum-remaining-posts.json),
   and the [complete pinned gist](upstream/org-roam-db-materialized-view.md).

## Original implementation materials

| Material | Local file |
|---|---|
| Original table and index definitions | [schema.sql](extracted/schema.sql) |
| Original nested join query, still an escaped Lisp string | [original-query.el.txt](extracted/original-query.el.txt) |
| View table, initial population, index and triggers | [materialized-view.sql](extracted/materialized-view.sql) |
| Standalone population snippet | [initial-population.sql](extracted/initial-population.sql) |
| Modified Emacs node-list function | [node-list.el](extracted/node-list.el) |
| Forum author's generated-note fixture script | [generate-notes.sh](extracted/generate-notes.sh) |
| Vulpea before / after implementation | [base](upstream/vulpea-base/vulpea-db.el), [head](upstream/vulpea-head/vulpea-db.el) |
| Vulpea benchmark source | [vulpea-perf-test.el](upstream/vulpea-head/test/vulpea-perf-test.el) |

Vulpea base is `5f5f2c892c99091a6d3703686754240ebd4936da`; PR head is
`ffd96eae7745c4c01ab5b47ef66e0c53b4627cd3`. The gist is pinned to
`192bafa002578f7691dda884f7bcc5f044dc721c`.

The trigger experiment retains LEFT JOINs, nested grouping, `group_concat`,
string formatting, nullable fields, foreign keys and insert/update/delete
triggers. Its materialized columns do not cover every relation mentioned in
the broader proposal. No query has been reduced to a count-only example.

## Evidence and gaps

The forum author's [final conclusions](extracted/benchmark-conclusions.md)
report an advantage for complex joins but a slight regression for joining the
view to files. They explicitly describe the measurements as best averages and
report substantially higher first-query latency. These are source reports,
not independently reproduced measurements.

The original benchmark code downloads the `vulpea-view-table` branch of
[vulpea-test-notes](https://github.com/d12frosted/vulpea-test-notes).
Resolving that branch returned HTTP 422 at collection time. The current master
commit is recorded in [test-notes-head.json](test-notes-head.json), but it is
not substituted for the historical data. The linked final benchmark sheet
returned HTTP 502. Both failures are recorded in the collection manifest.

The forum's generator is captured as an original synthetic fixture generator;
it is not the author's private notes or the 9,554-note dataset. It requires
Bash and `uuidgen`, then an Org-roam environment to populate the database.
The selected Vulpea sources are not a complete runnable Emacs package checkout.
No scripts, triggers or performance tests have been run in this collection.

Phase support: not evaluated.
