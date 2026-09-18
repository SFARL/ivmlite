# ivmlite 文档

## 目录

- **`superpowers/specs/`** — 设计文档（spec）。实施前的权威依据。
  - [`2026-09-18-ivmlite-design.md`](superpowers/specs/2026-09-18-ivmlite-design.md) — 总体设计
- **`superpowers/plans/`** — 实施计划。
  - [`2026-09-18-m0-test-and-bench-harness.md`](superpowers/plans/2026-09-18-m0-test-and-bench-harness.md) — M0：测试与基准骨架
- **`adr/`** — 架构决策记录（Architecture Decision Record）。
- **`spikes/`** — 可行性探针及其结论。产出是结论，不是要保留的代码。
  - [`2026-09-18-m-1-sqlite-extension-mechanics.md`](spikes/2026-09-18-m-1-sqlite-extension-mechanics.md) — M-1：控制面到底能不能按设想工作（阻塞 M1）

## ADR 的使用约定

**当前所有"为什么不是另一条路"的决定都记录在总体设计文档的 §12「决策记录：被否决的方案」中**，包括：

| 决策 | 位置 |
|---|---|
| 为什么目标是 SQLite 而不是 DuckDB | §12.1 |
| 为什么是扩展形态而不是"SQLite 旁边的库" | §12.2 |
| 为什么用 trigger 而不是 `preupdate_hook` / session extension | §12.3 |
| 为什么不走 SQL-to-SQL 编译路线 | §12.4 |
| 为什么不做静默全量回退 | §12.5 |
| 为什么 v0 不建完整 DBSP circuit | §12.6 |
| 为什么 DELETE 在 v0 而不是 v0.2 | §12.7 |
| 为什么不与 TanStack DB 对比 | §12.8 |

`adr/` 目录留给**spec 定稿之后**才出现的决策——即实施过程中推翻或新增的判断。届时每条一个文件，命名 `NNNN-<kebab-case-标题>.md`，并在本文件的表格中补一行。

这样做的原因：在 spec 尚未实施时把决策拆散到十几个文件里，只会让阅读顺序断裂；而实施期的决策必须独立记录，否则会悄悄改掉 spec 的前提而无人察觉。
