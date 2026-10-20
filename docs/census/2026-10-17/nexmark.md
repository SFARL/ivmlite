# Nexmark census against the ivmlite v0 subset

Corpus: nexmark/nexmark at `6b3646c3` (Flink SQL variant, `queries/q0.sql` to `q23.sql`), classified against the v0 subset in `taxonomy.md`. Schema mapping and the two bag workloads (Q0, Q20) are from the M2b preflight. Machine-readable data: `nexmark.json`.

Conventions: `minimal_unlock` is the smallest set of codes that makes the query run unchanged in meaning. `adapted_unlock` is what remains after the documented rewrite; `(none)` there means the adapted form already runs on v0. `n/a` means no faithful rewrite was found. F15 subtypes: `F15:ordered` blocks (order- or gap-dependent state), `F15:dedup` is an implementable set-semantics grouping, `F15:flink-fn` is a Flink-only function replaceable with SQLite built-ins and is therefore listed under features but excluded from `minimal_unlock`. All timestamps are INTEGER under the preflight's STRICT mapping, so F10b is only used for REAL-valued results.

## Per-query table

| Query | Shape | Features beyond v0 | minimal_unlock | adapted_unlock (rewrite cost) | fit | conf |
|---|---|---|---|---|---|---|
| Q0 | pass-through projection | F1 | F1 | (none) (bag) | fits_with | high |
| Q1 | currency conversion | F1, F4b, F10b | F1, F4b, F10b | (none) (stored-columns) | fits_with | high |
| Q2 | filter MOD(auction,123)=0 | F1, F6 | F1, F6 | (none) (stored-columns) | fits_with | high |
| Q3 | join + AND/OR filter | F1, F6 | F1, F6 | (none) (stored-columns) | fits_with | high |
| Q4 | MAX then AVG per category (nested agg) | F8, F3c, F3b, F6, F7c | F8, F3c, F3b, F6, F7c | n/a | fits_with | high |
| Q5 | HOP window count + max-join | F12, F1, F8, F3c, F7c, F6 | F1, F12, F8, F3c, F7c, F6 | F8, F3c, F7c, F6 (heavy) | out_of_scope | high |
| Q6 | winning bid + rolling AVG OVER | F9, F8, F3b, F6, F7c, F15 | F9, F8, F3b, F6, F7c, F15 | n/a | out_of_scope | low |
| Q7 | TUMBLE max + join back | F1, F12, F8, F3c, F7c, F6 | F1, F12, F8, F3c, F7c, F6 | F8, F3c, F7c, F6 (stored-columns) | out_of_scope | high |
| Q8 | two TUMBLE sets joined | F12, F1, F8, F7c, F15 | F1, F12, F8, F7c, F15 | (none) (stored-columns) | out_of_scope | high |
| Q9 | winning bid (ROW_NUMBER top1) | F9, F8, F6, F7c | F9, F8, F6, F7c | n/a | fits_with | high |
| Q10 | projection + DATE_FORMAT x2 | F1, F4b | F1, F4b | (none) (stored-columns) | fits_with | high |
| Q11 | SESSION window count | F12, F15 | F12, F15 | n/a | out_of_scope | high |
| Q12 | PROCTIME TUMBLE count | F12 | F12 | (none) (stored-columns) | out_of_scope | high |
| Q13 | temporal join with side input | F1, F12, F7c | F1, F12, F7c | (none) (stored-columns) | out_of_scope | high |
| Q14 | expressions, CASE, UDF, range filter | F1, F4b, F6, F10b, F15 | F1, F4b, F6, F10b | (none) (stored-columns) | fits_with | high |
| Q15 | daily stats, 12 aggs w/ FILTER+DISTINCT | F5, F3d, F4a | F5, F3d, F4a | F3d (stored-columns) | fits_with | high |
| Q16 | channel/day stats, + MAX | F5, F3d, F4a, F3c | F5, F3d, F4a, F3c | F3d, F3c (stored-columns) | fits_with | high |
| Q17 | auction/day stats, MIN/MAX/AVG | F5, F4a, F3c, F3b | F5, F4a, F3c, F3b | F3c, F3b (stored-columns) | fits_with | high |
| Q18 | last bid per (bidder,auction) | F9, F8 | F9, F8 | n/a | fits_with | high |
| Q19 | top-10 bids per auction | F9, F8 | F9, F8 | n/a | fits_with | high |
| Q20 | bid-auction join, category=10 | F1 | F1 | (none) (bag) | fits_with | high |
| Q21 | CASE/regexp/IN projection | F1, F4b, F6, F15 | F1, F4b, F6 | (none) (stored-columns) | fits_with | high |
| Q22 | SPLIT_INDEX projection | F1, F4b, F15 | F1, F4b | (none) (stored-columns) | fits_with | low |
| Q23 | 3-way join | F1, F7b | F1, F7b | F7b (bag) | fits_with | high |

## Per-query detail

### Q0 (pass-through projection)

- Source: `queries/q0.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (bag): Bag rewrite from the preflight (executed, q0_bag): SELECT all five columns plus COUNT(*) AS multiplicity ... GROUP BY all five columns. Expanding multiplicity reconstructs the bag; output shape and maintenance workload change.
- Adapted confidence: high.
- Confidence high: Pure pass-through projection; preflight error is exactly the missing aggregate/GROUP BY.
- Notes: NULL payload columns group together under GROUP BY, so the multiset is preserved (preflight checkpoints passed).

### Q1 (currency conversion)

- Source: `queries/q1.sql` at `6b3646c3`.
- Flink-specific constructs: DECIMAL(23,3) result type.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Bag rewrite (GROUP BY all projected columns + COUNT(*)) and the expression 0.908*price replaced by an app-maintained INTEGER column price_eur_milli = price*908 (exactly the DECIMAL(23,3) value scaled by 1000). Alternative without a stored column: bag rewrite + F5 (price*908 as a GROUP BY expression).
- Adapted confidence: high.
- Confidence high: One arithmetic projection over a single table; the preflight rejected `0.908 * price`.
- Notes: In SQLite 0.908*price is REAL (hence F10b/float); the scaled-integer column is exact because 0.908 has three decimals.

### Q2 (filter MOD(auction,123)=0)

- Source: `queries/q2.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Bag rewrite on (auction, price) + COUNT(*), with the predicate MOD(auction,123)=0 turned into an app-maintained INTEGER column auction_mod123 and `WHERE auction_mod123 = 0` (one col-op-literal predicate).
- Adapted confidence: high.
- Confidence high: Single-table filter with a scalar function in the predicate; no Flink-only construct.
- Notes: SQLite `%` truncates toward zero like Flink MOD; keys here are non-negative anyway. Preflight reported the first error as the aggregate-less root, the function-in-WHERE comes second.

### Q3 (join + AND/OR filter)

- Source: `queries/q3.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Bag rewrite on (name, city, state, A.id) + COUNT(*). The two filters are folded into nullable app-maintained join keys, which exploits the rule that a NULL join key matches nothing (spec 7.x, measured): auction.seller_if_cat10 = CASE WHEN category=10 THEN seller END and person.id_if_state_ok = CASE WHEN state IN ('OR','ID','CA') THEN id END; join ON seller_if_cat10 = id_if_state_ok with no WHERE.
- Adapted confidence (low): Correct by the NULL-key rule but untested here; the stored keys move the whole filter into the application and change the join-key columns.
- Confidence high: Join plus AND/OR predicates on two tables; nothing Flink-specific.
- Notes: v0 allows one WHERE predicate, so the AND of a category test and a three-way OR over state is F6.

### Q4 (MAX then AVG per category (nested agg))

- Source: `queries/q4.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (none): No light rewrite found: the inner MAX per auction must feed an outer AVG per category. Splitting into two chained views (inner view GROUP BY A.id, A.category with MAX; outer GROUP BY category with SUM and COUNT) still needs F8 in the form of view-over-view, which v0 does not demonstrate.
- Adapted confidence: low.
- Confidence high: Pure relational nested aggregation; the BETWEEN is on data columns, not a window.
- Notes: The join has an equi pair (A.id=B.auction) plus a range condition on two other columns (F7c non-equi, F6 BETWEEN with column bounds). MAX needs delete-aware state (F3c). SQLite AVG returns REAL whereas Flink returns BIGINT, so the output differs unless truncated (F4b).

### Q5 (HOP window count + max-join)

- Source: `queries/q5.sql` at `6b3646c3`.
- Flink-specific constructs: HOP(TABLE bid, DESCRIPTOR(dateTime), 2s, 10s) table function; window_start/window_end; watermark-driven window close.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (heavy): Replace HOP with an app-maintained window-membership table bid_win(auction, win_start, win_end) holding 5 rows per bid (10s window, 2s slide); bag rewrite at the root. The two window/group derived tables, the MAX, the (start,end) two-column join key and the num >= maxn non-equi condition remain.
- Adapted confidence: low.
- Confidence high: HOP table function is F12 by definition; the relational residue is a nested aggregate with a multi-column non-equi join.
- Notes: Core logic (per-window hottest auctions) is meaningful in SQLite given a membership table, but the answer then holds every window ever seen, not the window-close emissions of Flink. Fan-out of 5 changes the maintenance workload.

### Q6 (winning bid + rolling AVG OVER)

- Source: `queries/q6.sql` at `6b3646c3`.
- Flink-specific constructs: Query text is not valid Flink SQL as written (window alias rownum filtered at its own query level, B.price referenced outside its subquery); upstream marks it unsupported.
- Core logic meaningful in SQLite without them: yes.
- F15:ordered: ordered rolling frame AVG(...) OVER (PARTITION BY seller ORDER BY dateTime ROWS BETWEEN 10 PRECEDING AND CURRENT ROW) over a retracting input; the file itself carries a TODO saying Flink SQL does not support it.
- Adapted rewrite (none): None. Even a corrected form needs winning-bid top-1 per auction (ROW_NUMBER, F9), then a per-seller rolling average over the last 10 closed auctions ordered by time, which is an ordered window frame.
- Adapted confidence: low.
- Confidence low: The query is broken upstream, so the classification rests on the corrected intent (rolling frame with retractions), not on the text.
- Notes: Could be reclassified fits_with (all F9) if ivmlite ever implements generic window functions with retraction; ordered-frame maintenance is the blocking part.

### Q7 (TUMBLE max + join back)

- Source: `queries/q7.sql` at `6b3646c3`.
- Flink-specific constructs: TUMBLE(TABLE bid, DESCRIPTOR(dateTime), 10s) table function; window_start/window_end; watermark-driven window close.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Add an app-maintained INTEGER column win_end = (dateTime/10000+1)*10000 to bid (TUMBLE is [start,end), so floor division is exact); derived table B1 becomes SELECT MAX(price), win_end FROM bid GROUP BY win_end; keep the join `B.price = B1.maxprice AND B.dateTime BETWEEN B1.win_end-10000 AND B1.win_end` including its closed upper bound, so a bid exactly on a window boundary behaves as in the original; bag rewrite at the root.
- Adapted confidence: low.
- Confidence high: TUMBLE is F12; the residue is a derived-table MAX joined back with an equality plus a range predicate.
- Notes: 'Highest bid per 10s bucket' is meaningful in SQLite with a stored bucket. Flink emits per closed window; the SQLite view holds all buckets.

### Q8 (two TUMBLE sets joined)

- Source: `queries/q8.sql` at `6b3646c3`.
- Flink-specific constructs: TUMBLE over person and over auction; window_start/window_end; watermark-driven window close.
- Core logic meaningful in SQLite without them: yes.
- F15:dedup: GROUP BY with no aggregate on both derived tables (set/DISTINCT semantics).
- Adapted rewrite (stored-columns): Add win_start INTEGER = (dateTime/10000)*10000 to person and auction, plus a composite TEXT join key on each side (person.pk = id||':'||win_start, auction.sk = seller||':'||win_start) so the join is a single same-typed pair. Collapse both derived tables into the join and use the bag rewrite GROUP BY P.id, P.name, P.win_start + COUNT(*). Presence of a group equals presence of the original row; multiplicity becomes the number of auctions by that seller in the window instead of 1.
- Adapted confidence (low): Set semantics are lost; it is faithful only after collapsing multiplicity to presence, and it assumes person (id,name,window) is already unique.
- Confidence high: Two TUMBLE windows are F12; the join also needs a two-column key.
- Notes: 'People who both joined and opened an auction in the same 10s bucket' is meaningful in SQLite with stored buckets.

### Q9 (winning bid (ROW_NUMBER top1))

- Source: `queries/q9.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (none): None found; winning bid per auction needs a top-1 by (price DESC, dateTime ASC), which has no GROUP BY/SUM form in v0.
- Adapted confidence: low.
- Confidence high: Standard top-1-per-group over a range join; no Flink-only construct (BETWEEN is on data columns).
- Notes: Output is the full auction and bid row, so any maintained form is a bag/ranked result. Ties on (price, dateTime) are resolved arbitrarily.

### Q10 (projection + DATE_FORMAT x2)

- Source: `queries/q10.sql` at `6b3646c3`.
- Flink-specific constructs: Filesystem sink partitioned by (dt, hm) with partition-commit policy (sink side only, not part of the SELECT).
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Bag rewrite + app-maintained TEXT columns dt = strftime('%Y-%m-%d', dateTime/1000,'unixepoch') and hm = strftime('%H:%M', ...) (assumes epoch milliseconds, UTC).
- Adapted confidence: high.
- Confidence high: The SELECT is a projection plus two date-format scalars; the file-system sink is outside the view.
- Notes: DATE_FORMAT maps to strftime, so no new feature code beyond F4b. Timezone: Flink uses the session time zone.

### Q11 (SESSION window count)

- Source: `queries/q11.sql` at `6b3646c3`.
- Flink-specific constructs: SESSION(dateTime, 10s) group window, SESSION_START, SESSION_END; watermark-driven window close.
- Core logic meaningful in SQLite without them: no.
- F15:ordered: gap-based session merging: an insert inside a gap can merge two sessions and a delete can split one, so the grouping itself is order/gap dependent.
- Adapted rewrite (none): None. Sessionisation in plain SQL needs LAG/window functions and a cumulative sum over each bidder's ordered bids; one new bid can renumber every later session.
- Adapted confidence: high.
- Confidence high: Session windows are F12 and also a stateful ordered grouping that does not decompose into GROUP BY/SUM deltas.
- Notes: Never fits.

### Q12 (PROCTIME TUMBLE count)

- Source: `queries/q12.sql` at `6b3646c3`.
- Flink-specific constructs: PROCTIME() processing-time attribute (wall clock at arrival); TUMBLE(TABLE B, DESCRIPTOR(p_time), 10s) over a view; window_start/window_end.
- Core logic meaningful in SQLite without them: no.
- Adapted rewrite (stored-columns): Stamp each bid at insert time with INTEGER p_start (arrival time floored to 10s) and p_end = p_start+10000 (column DEFAULT or insert trigger), then GROUP BY bidder, p_start, p_end + COUNT(*).
- Adapted confidence (low): Result depends on when rows were inserted, not on row contents, so it is reproducible only because the stamp is stored; it is a different (arrival-time) semantics and meaningless as a replayable benchmark.
- Confidence high: Processing-time windows depend on the wall clock at arrival, which is F12 by the taxonomy.
- Notes: Bucket counts per bidder per arrival bucket is runnable on v0 once the stamp column exists.

### Q13 (temporal join with side input)

- Source: `queries/q13.sql` at `6b3646c3`.
- Flink-specific constructs: PROCTIME(); temporal join: side_input FOR SYSTEM_TIME AS OF B.p_time; join key is an expression mod(B.auction,10000).
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Bag rewrite on (auction, bidder, price, dateTime, value) + COUNT(*); an app-maintained INTEGER column auction_mod10000 on bid; ordinary inner join ON bid.auction_mod10000 = side_input.key. The temporal (as-of-arrival) lookup is replaced by a snapshot join, which is exact only if side_input never changes after the bids it joins.
- Adapted confidence (low): Equivalent only for a frozen side table; any later change to side_input would retroactively rewrite old joined rows, which the temporal join would not.
- Confidence high: A FOR SYSTEM_TIME AS OF temporal join is explicitly F12.
- Notes: Nexmark loads side_input once from a file, so the frozen assumption holds in the benchmark. The expression join key is the additional gap.

### Q14 (expressions, CASE, UDF, range filter)

- Source: `queries/q14.sql` at `6b3646c3`.
- Flink-specific constructs: Flink Java UDF count_char; HOUR(dateTime); DECIMAL(23,3) result type.
- Core logic meaningful in SQLite without them: yes.
- F15:flink-fn: count_char UDF (Java, counts bytes equal to the first byte of the argument); replaceable by length(CAST(x AS BLOB)) - length(CAST(replace(x,'c','') AS BLOB)) with NULL mapped to 0, so not blocking.
- Adapted rewrite (stored-columns): Bag rewrite + app-maintained columns: price_eur_milli = price*908 (INTEGER), bid_time_type (CASE over the hour), c_counts (byte count of 'c' in extra, 0 for NULL), and a 0/1 flag in_range = (price*908 > 1000000000 AND price*908 < 50000000000) replacing the two expression predicates with `WHERE in_range = 1`.
- Adapted confidence (low): Every computed output and the filter live in stored columns, so the view merely stores what the application computes; the integer scaling is exact.
- Confidence high: Expressions, CASE, a UDF and an AND of two expression predicates over a single table.
- Notes: The comparison bounds scale by 1000 along with the stored milli price.

### Q15 (daily stats, 12 aggs w/ FILTER+DISTINCT)

- Source: `queries/q15.sql` at `6b3646c3`.
- Flink-specific constructs: FILTER (WHERE ...) clauses on aggregates (a standard-SQL feature SQLite 3.30+ also supports).
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): App-maintained day TEXT column; three 0/1 flag columns r1,r2,r3 for the price bands so `count(*) filter (where band)` becomes SUM(r1) etc. (v0 SUM over INTEGER); stored nullable columns bidder_r1/_r2/_r3 and auction_r1/_r2/_r3 (= bidder or auction when in the band, else NULL) so each filtered distinct count becomes COUNT(DISTINCT bidder_rN). The three unfiltered distinct counts remain COUNT(DISTINCT col): F3d stays.
- Adapted confidence (low): Needs 12 extra stored columns; F3d (distinct state) cannot be rewritten into v0 without chained views.
- Confidence high: Day bucketing, FILTER aggregates and COUNT(DISTINCT) are all ordinary SQL.
- Notes: DATE_FORMAT('yyyy-MM-dd') maps to strftime, an F5 expression in GROUP BY.

### Q16 (channel/day stats, + MAX)

- Source: `queries/q16.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): As Q15 (stored day, band flags, nullable per-band bidder/auction columns), plus a stored TEXT column minute = HH:mm so max(minute) becomes MAX(minute), still F3c (MAX on TEXT).
- Adapted confidence (low): Same heavy stored-column preparation as Q15; F3d and F3c remain.
- Confidence high: Same shape as Q15 plus a MAX over a derived string.
- Notes: channel is a bare TEXT group key; the day is an expression key (F5).

### Q17 (auction/day stats, MIN/MAX/AVG)

- Source: `queries/q17.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (stored-columns): Stored day TEXT column; 0/1 band flags so filtered counts are SUM(r1) etc.; COUNT(*) and SUM(price) already in v0. MIN, MAX and AVG remain.
- Adapted confidence: high.
- Confidence high: Group by one bare column plus a day expression with ten ordinary aggregates.
- Notes: AVG(price) is BIGINT in Flink and REAL in SQLite; MIN/MAX/AVG need delete-aware or SUM/COUNT-derived state. Preflight notes that reducing to COUNT/SUM changes the workload.

### Q18 (last bid per (bidder,auction))

- Source: `queries/q18.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (none): None found: latest row per (bidder, auction) needs either ROW_NUMBER or MAX(dateTime) joined back, both outside v0.
- Adapted confidence: low.
- Confidence high: Deduplication by ROW_NUMBER is plain SQL; dateTime is just a data column here.
- Notes: Ties on dateTime within a partition are resolved arbitrarily.

### Q19 (top-10 bids per auction)

- Source: `queries/q19.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (none): None found: top-10 per auction by price is a ranked result.
- Adapted confidence: low.
- Confidence high: Top-N per partition via ROW_NUMBER is plain SQL.
- Notes: Output also includes the rank column. Ties on price are resolved arbitrarily.

### Q20 (bid-auction join, category=10)

- Source: `queries/q20.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (bag): Bag rewrite from the preflight (executed, q20_bag): alias the duplicate dateTime and extra columns (bid_dateTime, bid_extra, auction_dateTime, auction_extra) and add COUNT(*) AS multiplicity with GROUP BY every output field. Aliasing only renames columns that the Flink sink also names positionally.
- Adapted confidence: high.
- Confidence high: Single equi-join with one col=literal filter; only the aggregate-less root and duplicate output names block it.
- Notes: The duplicate-name rejection is an ivmlite output-column rule, not a missing SQL feature.

### Q21 (CASE/regexp/IN projection)

- Source: `queries/q21.sql` at `6b3646c3`.
- Flink-specific constructs: REGEXP_EXTRACT.
- Core logic meaningful in SQLite without them: yes.
- F15:flink-fn: REGEXP_EXTRACT(url,'(&|^)channel_id=([^&]*)',2); replaceable by instr/substr on '&'||url (leftmost match semantics are preserved), so not blocking.
- Adapted rewrite (stored-columns): Bag rewrite on (auction, bidder, price, channel, channel_id) + COUNT(*) with app-maintained TEXT column channel_id (the CASE/lower/extract expression) and a 0/1 column keep (the OR/IN predicate, including the regexp-not-null test), then `WHERE keep = 1`.
- Adapted confidence (low): The stored columns carry the whole query logic, so the view reduces to a bag over precomputed values.
- Confidence high: CASE, lower(), string extraction and an OR/IN predicate over one table.
- Notes: SQLite lower() is ASCII-only; Flink lower() is Unicode-aware.

### Q22 (SPLIT_INDEX projection)

- Source: `queries/q22.sql` at `6b3646c3`.
- Flink-specific constructs: SPLIT_INDEX.
- Core logic meaningful in SQLite without them: yes.
- F15:flink-fn: SPLIT_INDEX(url,'/',n); no SQLite built-in, expressible with nested instr/substr (NULL when the index is out of range), so not blocking.
- Adapted rewrite (stored-columns): Bag rewrite on (auction, bidder, price, channel, dir1, dir2, dir3) + COUNT(*) with three app-maintained TEXT columns computed by the application from url.
- Adapted confidence (low): All logic is in stored columns.
- Confidence low: Hinges on whether a Flink-only function counts as F15 out_of_scope or a plain F4b expression; the logic itself is a trivial projection.
- Notes: Under a strict reading of the taxonomy (F15 'Flink-only function SPLIT_INDEX') this would be out_of_scope; here it is treated as a dialect function. The same reading applies to Q21's REGEXP_EXTRACT and Q14's count_char.

### Q23 (3-way join)

- Source: `queries/q23.sql` at `6b3646c3`.
- Flink-specific constructs: none.
- Core logic meaningful in SQLite without them: yes.
- Adapted rewrite (bag): Bag rewrite (GROUP BY all 21 output fields + COUNT(*), with the aliases the query already has). The three-table join remains; materialising person JOIN auction in the application would defeat the purpose.
- Adapted confidence: high.
- Confidence high: Plain inner joins over three tables; the preflight rejected the table count.
- Notes: Both joins share the key bid.bidder (P.id = B.bidder, A.seller = B.bidder), so the join graph is a star around bid.

## Aggregates

Counts out of 24. `needs` counts every query listing the code in `features`; `minimal` counts queries whose `minimal_unlock` contains it; `alone` counts queries that become runnable if only that code is added (as written); `adapted in unlock` counts queries whose `adapted_unlock` contains it; `adapted alone` is the same for adapted forms whose whole unlock is that single code.

| Code | needs | minimal | alone (as written) | adapted in unlock | adapted alone |
|---|---|---|---|---|---|
| F1 | 14 | 14 | 2  (Q0, Q20) | 0 | 0  |
| F6 | 9 | 9 | 0  | 2 | 0  |
| F8 | 8 | 8 | 0  | 2 | 0  |
| F7c | 7 | 7 | 0  | 2 | 0  |
| F12 | 6 | 6 | 0  | 0 | 0  |
| F3c | 5 | 5 | 0  | 4 | 0  |
| F4b | 5 | 5 | 0  | 0 | 0  |
| F9 | 4 | 4 | 0  | 0 | 0  |
| F3b | 3 | 3 | 0  | 1 | 0  |
| F4a | 3 | 3 | 0  | 0 | 0  |
| F5 | 3 | 3 | 0  | 0 | 0  |
| F10b | 2 | 2 | 0  | 0 | 0  |
| F15:ordered | 2 | 2 | 0  | 0 | 0  |
| F3d | 2 | 2 | 0  | 2 | 1  (Q15) |
| F15:dedup | 1 | 1 | 0  | 0 | 0  |
| F7b | 1 | 1 | 0  | 1 | 1  (Q23) |
| F15:flink-fn | 3 | 0 | 0  | 0 | 0  |

Queries by code (minimal_unlock):

- F1: Q0, Q1, Q2, Q3, Q5, Q7, Q8, Q10, Q13, Q14, Q20, Q21, Q22, Q23
- F6: Q2, Q3, Q4, Q5, Q6, Q7, Q9, Q14, Q21
- F8: Q4, Q5, Q6, Q7, Q8, Q9, Q18, Q19
- F7c: Q4, Q5, Q6, Q7, Q8, Q9, Q13
- F12: Q5, Q7, Q8, Q11, Q12, Q13
- F3c: Q4, Q5, Q7, Q16, Q17
- F4b: Q1, Q10, Q14, Q21, Q22
- F9: Q6, Q9, Q18, Q19
- F3b: Q4, Q6, Q17
- F4a: Q15, Q16, Q17
- F5: Q15, Q16, Q17
- F10b: Q1, Q14
- F15:ordered: Q6, Q11
- F3d: Q15, Q16
- F15:dedup: Q8
- F7b: Q23
- F15:flink-fn: (none)

### Fit as written

fits_now: 0. fits_with: 17. out_of_scope: 7. not_ivm_shaped: 0.

### Queries runnable on v0 today after the documented rewrite (zero new codes)

Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 (12). Only Q0 and Q20 were executed by the preflight; the others are untested rewrites.

No faithful rewrite found: Q4, Q6, Q9, Q11, Q18, Q19.

### Bundles of 2 and 3 codes

A query counts for a bundle when its whole unlock set is inside the bundle and it is not blocked by F12 or F15:ordered. For adapted forms the table reports the marginal gain over the 12 queries already runnable with zero codes. Ties are broken alphabetically, so other equal bundles exist.

Best pairs as written:

| Bundle | n | Queries |
|---|---|---|
| F1 + F4b | 4 | Q0, Q10, Q20, Q22 |
| F1 + F6 | 4 | Q0, Q2, Q3, Q20 |
| F1 + F7b | 3 | Q0, Q20, Q23 |
| F1 + F10b | 2 | Q0, Q20 |
| F1 + F15:dedup | 2 | Q0, Q20 |
| F1 + F15:flink-fn | 2 | Q0, Q20 |

Best pairs adapted (marginal over zero-code baseline):

| Bundle | marginal | Queries gained | Total adapted runnable |
|---|---|---|---|
| F3c + F3d | 2 | Q15, Q16 | 14 |
| F3d + F7b | 2 | Q15, Q23 | 14 |
| F1 + F3d | 1 | Q15 | 13 |
| F1 + F7b | 1 | Q23 | 13 |
| F3b + F3c | 1 | Q17 | 13 |
| F3b + F3d | 1 | Q15 | 13 |

Best triples as written:

| Bundle | n | Queries |
|---|---|---|
| F1 + F4b + F6 | 7 | Q0, Q2, Q3, Q10, Q20, Q21, Q22 |
| F1 + F4b + F10b | 5 | Q0, Q1, Q10, Q20, Q22 |
| F1 + F4b + F7b | 5 | Q0, Q10, Q20, Q22, Q23 |
| F1 + F6 + F7b | 5 | Q0, Q2, Q3, Q20, Q23 |
| F1 + F3b + F4b | 4 | Q0, Q10, Q20, Q22 |
| F1 + F3b + F6 | 4 | Q0, Q2, Q3, Q20 |

Best triples adapted (marginal over zero-code baseline):

| Bundle | marginal | Queries gained | Total adapted runnable |
|---|---|---|---|
| F3b + F3c + F3d | 3 | Q15, Q16, Q17 | 15 |
| F3c + F3d + F7b | 3 | Q15, Q16, Q23 | 15 |
| F1 + F3c + F3d | 2 | Q15, Q16 | 14 |
| F1 + F3d + F7b | 2 | Q15, Q23 | 14 |
| F3b + F3c + F7b | 2 | Q17, Q23 | 14 |
| F3b + F3d + F7b | 2 | Q15, Q23 | 14 |

Named bundles (all queries inside the bundle, including those unlocked by smaller sets):

| Bundle | As written | Adapted (total) |
|---|---|---|
| F1+F4b | 4: Q0, Q10, Q20, Q22 | 12: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 |
| F1+F6 | 4: Q0, Q2, Q3, Q20 | 12: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 |
| F1+F4b+F6 | 7: Q0, Q2, Q3, Q10, Q20, Q21, Q22 | 12: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 |
| F1+F4b+F6+F10b | 9: Q0, Q1, Q2, Q3, Q10, Q14, Q20, Q21, Q22 | 12: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 |
| F1+F7b | 3: Q0, Q20, Q23 | 13: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22, Q23 |
| F9+F8 | 2: Q18, Q19 | 12: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q20, Q21, Q22 |
| F5+F4a+F3d | 1: Q15 | 13: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q15, Q20, Q21, Q22 |
| F5+F4a+F3c+F3b | 1: Q17 | 13: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q17, Q20, Q21, Q22 |
| F3c+F3b+F3d (adapted aggregates) | 0: - | 15: Q0, Q1, Q2, Q3, Q8, Q10, Q12, Q13, Q14, Q15, Q16, Q17, Q20, Q21, Q22 |
| F8+F3c+F7c+F6 (+F3b) | 1: Q4 | 15: Q0, Q1, Q2, Q3, Q5, Q7, Q8, Q10, Q12, Q13, Q14, Q17, Q20, Q21, Q22 |

### Queries that can never fit (F12 or a blocking F15)

- Q5 (HOP window count + max-join): F12; adapted form possible (heavy, remaining codes: F8, F3c, F7c, F6).
- Q6 (winning bid + rolling AVG OVER): F15:ordered; adapted form n/a.
- Q7 (TUMBLE max + join back): F12; adapted form possible (stored-columns, remaining codes: F8, F3c, F7c, F6).
- Q8 (two TUMBLE sets joined): F12; adapted form possible (stored-columns, remaining codes: (none)).
- Q11 (SESSION window count): F12+F15:ordered; adapted form n/a.
- Q12 (PROCTIME TUMBLE count): F12; adapted form possible (stored-columns, remaining codes: (none)).
- Q13 (temporal join with side input): F12; adapted form possible (stored-columns, remaining codes: (none)).

Of these, Q5, Q7, Q8, Q12 and Q13 have a core that is meaningful in SQLite once the window or temporal construct is replaced by stored data (bucket column, membership table, frozen side table); Q11 (SESSION) and Q6 (ordered rolling frame) do not.

## Caveats

- `fit` is judged on the taxonomy: any F12 query is out_of_scope as written even where a bucket column rescues it. The adapted column shows that rescue.
- Q22 (SPLIT_INDEX), Q21 (REGEXP_EXTRACT) and Q14 (count_char) are tagged F15:flink-fn but not blocking, since SQLite built-ins reproduce them. Under a strict reading of the taxonomy example, Q22 would be out_of_scope; Q21 and Q14 would too.
- Stored-column rewrites move the query's computation into the application, so for Q10, Q14, Q21, Q22 the adapted view only stores precomputed values. Treat them as workloads, not as feature coverage.
- Q6 is invalid Flink SQL as written (upstream TODO), so its classification is low confidence. Several rewrites (Q3, Q8, Q12, Q13) are unexecuted and rely on stated assumptions (NULL join keys match nothing, set-to-multiplicity collapse, frozen side input).
- AVG semantics differ: Flink AVG over BIGINT yields BIGINT, SQLite AVG yields REAL (Q4, Q17). Whether views over views are allowed (an alternative to F8) is not shown in the spec.
- Whether app-maintained columns can be SQLite STORED generated columns under ivmlite was not checked.
