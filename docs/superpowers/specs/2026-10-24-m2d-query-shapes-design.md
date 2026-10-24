# M2d: query shapes

**Scope:** the user-selected next milestone: aggregate argument expressions,
static AND filters, nonaggregated row bags, and SELECT expressions. This is core /
SQLite extension work, not Kener queue or Redis integration. Work starts from
`e443c68` on `codex/m2d-query-shapes`; preserve the preceding PR's source/evidence.

## SQL contract

The initial deterministic scalar grammar is:

```text
scalar := column | SQL literal | '(' scalar ')'
        | '+' scalar | '-' scalar
        | scalar ('+' | '-' | '*' | '/' | '%') scalar
atom   := existing bare-column/literal comparison | column IS [NOT] NULL
where  := atom | '(' where ')' | where AND where
```

Literals retain existing INTEGER/REAL/TEXT/BLOB/NULL parsing rules. In particular,
`-9223372036854775808` remains INTEGER, rather than first parsing the magnitude
as REAL. Parameters, clock functions, arbitrary function calls, CASE, CAST,
collations, subqueries, expression group keys, OR/IN/NOT/BETWEEN and expression
comparisons in WHERE remain rejected explicitly. This does not claim complete
census F4a/F4b/F6 support.

Use scalar expressions as arguments to COUNT, SUM, TOTAL, AVG, MIN and MAX;
COUNT(*) remains special. Never rewrite SUM(a+b) into SUM(a)+SUM(b).
Aggregates retain the existing exact-accumulator and documented REAL/overflow
contract; only the per-row argument computation is new.

Row views have neither GROUP BY nor aggregates. They preserve SQL bag
multiplicity, including identical rows and collisions after projection. Support
ordinary SELECT expressions, aliases, SELECT * and qualified table wildcards
for row views. Duplicate result names require explicit distinct aliases, as
before. Grouped queries keep bare GROUP BY columns and reject global aggregates
or aggregate-free grouping. Their SELECT list may reorder/omit keys, repeat a
source value under distinct aliases, and contain arithmetic over grouped keys
and aggregate results. Ungrouped bare source references and nested aggregates
are rejected. Final projection collisions preserve multiple copies.

Examples:

```sql
SELECT user_id, SUM(up_bytes + down_bytes) AS bytes
FROM traffic WHERE active = 1 AND deleted_at IS NULL GROUP BY user_id;

SELECT bidder, price * 0.908 AS converted FROM bid WHERE price > 0 AND bidder IS NOT NULL;

SELECT SUM(bytes) + 1 AS adjusted, user_id FROM traffic GROUP BY user_id;
```

Retain the existing single ordinary-table or two-table, same-affinity inner
equijoin source contract. No multi-table/outer/self/non-equi join expansion.
Every accepted AND leaf is validated and affinity-converted, on either join
side, before a plan is produced. WHERE retains a row only when all leaves pass;
FALSE and UNKNOWN may collapse for filtering only, never for general Boolean
SELECT expressions.

## Expressions and evaluation

Introduce a resolved scalar AST with column indices, typed literals, unary and
binary arithmetic. Keep legacy Project and Agg.column layouts. Add a stateless
Map node, before Aggregate for argument values and after Aggregate for final
SELECT expressions. Preserve legacy canonical encodings and aggregate layouts.
The host numeric-parser fix below can intentionally change adversarial REAL
literal bits, including in otherwise legacy SQL.
New canonical encodings must include exact typed literals, operator and operand
order, and unambiguous length boundaries. Validate indices and bound recursive
expression/conjunction depth at the SQL/system boundary with a clear diagnostic.

Core exposes an explicit projection evaluator seam. The SQLite backend renders
only the validated AST to a parameterized SELECT, evaluates the entire projected
row together, and reuses cached statements/templates by layout. All values are
bound; no source SQL substring, identifier, function or database read enters
this expression SELECT. This uses the actual host SQLite's scalar coercion,
NULL propagation, integer overflow promotion, integer division, remainder,
unary-plus no-op and NaN-to-NULL behavior. Do not reuse aggregate numeric_term
as an arithmetic implementation.

The extension uses this evaluator during bootstrap and every delta computation,
including expressions over joined rows and aggregate results. Failures occur
before durable apply. Legacy core Node::delta continues to execute old plans;
if a plan needs the explicit evaluator, refuse it before traversing any child,
so a rejected call cannot mutate a child Join/Aggregate. Generic core ViewQuery
and frozen workload formats need not gain a second SQLite scalar implementation.
New Map plans use the explicit evaluator API.

### Review amendment: host numeric literal parsing

Rust's correctly rounded decimal parser can differ from the host SQLite's
literal parser by an ULP, even before arithmetic. For example,
`1.00000000000000011102230246251565404236316680908203126` reproduced that
difference. The SQL catalog therefore provides a REAL-literal parsing hook;
the native catalog converts the validated numeric spelling through a bound
`SELECT CAST(? AS REAL)` on the host. Integers, including the minimum i64,
remain INTEGER. Overflow-to-REAL numeric tokens use the same hook. Resolved
AST literals remain typed values, and their canonical bits bind the host's
interpretation; no raw user SQL is interpolated into the evaluator. Generic
non-SQLite catalogs may use the default Rust parser. Test adversarial long
decimals, subnormals, overflow and rounding boundaries on the bundled host and
minimum-supported runtime.

### Review amendment: coherent grouped keys

The existing aggregate path independently prefers REAL for each numerically
equal group-key column. That remains the legacy contract, but using such a
synthetic tuple in final arithmetic is unsafe: `(1.0,1)` and `(1,1.0)` can form
`(1.0,1.0)`, although neither source row has those storage classes together.
For `a/2+b/2`, the synthetic tuple gives 1.0 while either live tuple gives 0.5.

New grouped Map plans therefore retain an additional arrangement of complete,
typed key tuples and their live counts. Both the old output and its replacement
use one coherent live tuple for all group-key references; deleting the chosen
tuple must retract the old final expression and select a surviving tuple.
This representative may differ from SQLite's choice among numerically equal
INTEGER/REAL keys, including SQLite's single MIN/MAX extremal-row rule.
Type-sensitive arithmetic can therefore differ numerically in that mixed-key
case; it is not an exact SQLite-recomputation claim. Emitting a combination that
never existed is forbidden. Select the least exact typed tuple in `Row` order
deterministically. Legacy aggregate plans and persisted diagnostics retain their
existing encoding/layout, subject to the numeric-literal fix above. Test the two-tuple counterexample, representative
changes, duplicates, deletion of either tuple, rollback and reopen.

## Durable bag output

Keep one physical output row per SQL copy and __w=1, retaining stable physical
rowids and the current owned cursor snapshot behavior. Do not compact copies
into weights or invent duplicate virtual rowids.

For final bag roots (row projection or projection above Aggregate), add a hidden
encoded exact-Row identity column and nonunique index. INTEGER 1 and REAL 1.0
must have different identities, even though SQLite IS equates them. Reserve the
new internal name case-insensitively in result aliases. Keep the public output
column index and typeless value columns. Legacy root-Aggregate output can retain
its current path unchanged.

Stage one negative signed count per distinct encoded output Row; reject
i64::MIN before negation. Positive counts can be expanded into physical-copy
stage rows with the existing bounded Stager, since output itself contains those
copies. Do not expand negative counts into identical LIMIT-1 selections.

A dedicated bag-retraction trigger may run from the main apply trigger:

```sql
-- Main arming trigger, after buffered state application:
UPDATE stage SET op = 'bag-' WHERE op = 'out-';

-- Dedicated AFTER UPDATE OF op trigger, WHEN NEW.op = 'bag-':
SELECT RAISE(ABORT, 'ivmlite output retraction exceeds stored copies')
WHERE (SELECT count(*) FROM output WHERE identity_key = NEW.key) < -NEW.w;
DELETE FROM output WHERE rowid IN (
  SELECT rowid FROM output WHERE identity_key = NEW.key LIMIT -NEW.w
);
```

The actual code uses reserved/quoted owned names. A planning probe on SQLite
3.42.0 verified multi-copy typed deletion and atomic underflow with
recursive_triggers both OFF and ON; acceptance must repeat it on bundled and
minimum-supported SQLite. Each staged negative identity appears once, so these
deletions cannot compete for the same physical copies. The main trigger then
inserts positive copies and updates progress/GC. Everything remains in the
single final arming UPDATE; no fallible operation follows durable apply.

Verify the extra owned trigger/index on reopen and refresh. DROP cleans them
with the stage/output tables; missing or forged ownership must fail clearly.
Use **format 7** for this new durable protocol. Format 6 is explicitly refused
without mutation; old broken views remain droppable so hosts can rebuild from
base rows. No silent upgrade and no change to published alpha.6 artifacts.

## Public output and reads

Retain the current typeless/no-affinity virtual output contract; inferred
storage types do not become declared affinities. Native SQLite VIEW affinity
inheritance is not introduced. Preserve existing selective-read behavior for
legacy root Aggregate. New bag/final-expression roots initially use the safe
full-snapshot fallback. SQLite still performs residual filtering, ordering and
LIMIT; no new pushdown eligibility is inferred from source column types.

## Acceptance and Nexmark gate

Compare full typed multisets against ordinary SQLite recomputation. Include:

- All scalar operators, precedence and parentheses, signed literals, NULL,
  INTEGER/REAL/TEXT/BLOB, numeric prefixes and malformed input, embedded NUL,
  >2^53, i64 boundaries/overflow, division/modulo by zero, infinities and NaN.
- Direct dark-xray SUM(up_bytes+down_bytes), all aggregate functions over
  expressions, argument sharing and post-join evaluation; preserve documented
  exact-aggregate versus SQLite last-bit differences rather than normalizing
  arbitrary mismatches away.
- AND truth combinations, nested/repeated/contradictory leaves, each affinity,
  reversed comparisons and both join sides, with filter-crossing updates.
- Duplicate bootstrap, projection collisions, +N/-N, partial deletion and
  same-batch cancellation; INTEGER/REAL identity, NULLs, joined fanout and
  both-side changes; post-aggregate collisions and hidden group keys.
- Rollback/savepoints, failure and retry, reconnect with pending deltas,
  sibling/shared captures, drop/recreate, exact progress/state/GC preservation
  on failure, missing bag objects and format 6 refusal/drop/rebuild.
- Existing selective-read regressions plus explicit safe fallback for row and
  post-aggregate-expression roots.

Run scripts/test-all.sh so extension tests load the freshly rebuilt library;
then fmt/clippy, focused meaningful mutation controls, minimum SQLite smoke,
and latest-head Linux/macOS CI. Keep one local native build/test owner at a time.

Reprobe the pinned Nexmark corpus query-by-query. Preserve original SQL and
digests; record exact SQLite ports and every dialect or semantic adaptation.
Q0 should gain true bag projection; Q20 still needs unique result aliases;
Q1 multiplication needs explicit REAL-versus-DECIMAL semantics. Q2/Q3 and
Q10/Q14/Q21/Q22 retain additional predicate/function requirements. The historical
7–9 projection is a target for the broader bundle, not a claim that narrow AND
and arithmetic already execute all those original Flink queries. No M2b
performance claim or renewed Turso equivalence follows from this milestone.
