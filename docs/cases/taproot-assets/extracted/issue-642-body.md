## Architecture Issues

Today we have an event-sourced architecture on disk to keep track of events related to new proofs and syncs of assets in the Universe db. This works ok, but things really start to break down once the table gets more that 10 million or so rows. There're two primary issues with the current approach:
  1. Each proof query launches a new goroutine to append to the log. This can really start to build up, particularly with a ton of spammy concurrent clients and no rate limiting. 
  2. We use a non-materialized view to obtain query aggregate stats. This mean doing several joins over a table with tens of millions of rows. 

In short, without some architectural changes the current approach isn't very scalable. 

In #626 we stop automatically logging each sync to curb down on the DB row expansion. 

## Potential Solutions 

### Log Compaction 

If we want to keep with the log architecture (which is nice, as we can generate time series data that summarizes changes in the log data), then we need to reduce the total number of rows in the DB. We can do this by adding a background _compaction_ goroutine. We'd create a new sub-system, that would compact in the background. As an example if 10k log entries in the past hour resulted in a net delta of 100 assets and 10 new syncs, then we'd replace all 10k of those entries with a single entry that recorded just that delta. 

By setting the compaction threshold, compaction chunk-size, and interval properly, then we'd retain the benefits of the current arch, w/o having to worry about wrangling with 10s of millions of rows. 

### Materialized Views 

The main view we use for the stats isn't [materialized](https://www.postgresql.org/docs/current/rules-materializedviews.html): https://github.com/lightninglabs/taproot-assets/blob/e569cf553d324297c303d190678320fcc50b9c10/tapdb/sqlc/migrations/000010_universe_stats.up.sql#L3-L13. As a result, the query runs each time from scratch. A materialized view on the other hand, is actually just another table, so we'd query the table directly, instead of joining over millions of rows to find a single response.

These views need to periodically refreshed using the `REFRESH MATERIALIZED VIEW mymatview;` query. We'd set up a background job to this. 

This solves the issue of needing to do fresh joins with each query, but doesn't address the issue of tens of millions of rows. We can combine this with the above compaction idea to tackle both issues. 

### Scrap Event-Sourcing - Replace w/ Atomic Counters

If we want to abandon the event sourced architecture all together, then we can instead move to using atomic in memory counters that are periodically flushed to disk. We'd be able to retain the stats API calls, and even the event call with some additional book keeping (add a summary row each day). 

We can use a migration to run the stats query as is, then insert new rows, for each asset that summarize the final state. We can then maintain a scratch row, and use _that_ to insert a new row for each day. We can also adopt the materialized view approach here as well, tho that will still need to crunch through all the various rows. 



