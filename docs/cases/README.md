# Real-world case collection

Collected on 2026-09-25. These are original problem reports and implementation
materials, collected before deciding which ivmlite phase could support them.
They are **not yet replayed or evaluated against ivmlite**.

| Case | Original problem | Materials captured | Main missing material |
|---|---|---|---|
| [Org-roam](org-roam/README.md) | Repeated joins to assemble complete note records | Issue and comments, 28 forum posts, schema, original query, triggers, note generator, pinned Vulpea code and performance tests | Historical benchmark dataset and complete runnable Emacs environment |
| [Datasette](datasette/README.md) | Repeated facet queries, slow table counts, expensive facet suggestions | Six issues and comments, SQLite forum thread, original SQL, traces and reproduction commands | Historical databases and write traces |
| [Taproot Assets](taproot-assets/README.md) | Aggregating a growing universe event log | Issue and comments, all ten migrations at the cited commit, event-write and stats-read queries | Production data, workload distribution and timing baseline |
| [Zero](zero/README.md) | Concurrent view maintenance with interleaved reads and writes | Original transaction/concurrency requirement | Schema, queries, input transactions and expected outputs |
| [SQLite IVM request](sqlite-ivm-request/README.md) | Generic incremental view maintenance within SQLite | Original forum discussion | Concrete application workload |

The Datasette reports are separate subcases. The Org-roam proposal, Vulpea
implementation and later trigger experiment also have different coverage.
Do not combine them into a fictional single workload.

## How to read the collection

Start with a case's README, then its `extracted/` or `upstream/` files. The JSON
and HTML snapshots retain the surrounding discussion, including objections,
alternatives and negative performance results. Issue states are snapshot states,
not a live status feed. Reported timings belong to the original authors; none
are ivmlite measurements.

- [manifest.json](manifest.json) records source URLs, retrieval times, hashes,
  sizes, HTTP pagination metadata and failed retrievals. Downloaded responses
  are preserved byte for byte. Repository source files use immutable commits.
- [extractions.json](extractions.json) records exact extraction steps and hashes.
  Snippets preserve original spelling, whitespace, line endings and bugs. SQL
  strings embedded in another language remain strings; scripts are not executed.
- A source being captured does not imply its linked images, attachments,
  dependencies or datasets were also captured. Each dossier states the gaps.
- Third-party material retains its original authorship and terms; see
  [NOTICE.md](NOTICE.md).

Verify offline from the repository root:

```sh
python3 docs/cases/verify.py
```

This checks snapshot hashes, exact snippet extraction, GitHub comment counts,
PR file counts and Discourse post coverage. It does not execute upstream code
or establish SQL correctness or reproducibility.

## Later phase evaluation

Keep this corpus independent of the [simplified demos](../demos/README.md).
Those demos are not reproductions of these full applications.

For each later evaluation, record the original case and query, ivmlite phase
and commit, dataset origin, update sequence, required output semantics,
correctness comparison, read/write costs and unresolved requirements. Keep any
adapted query or synthetic fixture alongside its explicit differences from the
original. Support assessments and phase assignments are intentionally pending.
