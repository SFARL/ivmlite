I have a SQLite DB that's pretty large, 23GB and something like 300 million rows. I expect that most queries I run on it will be slow, which is fine, but there are some things that Datasette does that makes working with the DB very slow. Specifically, when I access the `.json` metadata for a table (which I believe it comes from `datasette/views/database.py`, it takes 43 seconds for the request to come in:

```bash
$ time curl localhost:9999/out.json
{"database": "out", "size": 24291454976, "tables": [{"name": "PageviewsHour", "columns": ["file", "code", "page", "pageviews"], "primary_keys": [], "count": null, "hidden": false, "fts_table": null, "foreign_keys": {"incoming": [], "outgoing": [{"other_table": "PageviewsHourFiles", "column": "file", "other_column": "file_id"}]}, "private": false}, {"name": "PageviewsHourFiles", "columns": ["file_id", "filename", "sha256", "size", "day", "hour"], "primary_keys": ["file_id"], "count": null, "hidden": false, "fts_table": null, "foreign_keys": {"incoming": [{"other_table": "PageviewsHour", "column": "file_id", "other_column": "file"}], "outgoing": []}, "private": false}, {"name": "sqlite_sequence", "columns": ["name", "seq"], "primary_keys": [], "count": 1, "hidden": false, "fts_table": null, "foreign_keys": {"incoming": [], "outgoing": []}, "private": false}], "hidden_count": 0, "views": [], "queries": [], "private": false, "allow_execute_sql": true, "query_ms": 43340.23213386536}
real	0m43.417s
user	0m0.006s
sys	0m0.016s
```

I suspect this is because a `COUNT(*)` is happening under the hood, which, when I run it through sqlite directly, does take around the same time:

```bash
$ time sqlite3 out.db < <(echo "select count(*) from PageviewsHour;")
362794272

real	0m44.523s
user	0m2.497s
sys	0m6.703s
```

I'm using the `.json` request in the [Observable Datasette Client](https://observablehq.com/@asg017/datasette-client) to 1) verify that a link passed in is a reachable Datasette instance, and 2) a quick way to look at metadata for a db. A few different solutions I can think of:

1. Have some other endpoint, like `/-/datasette.json` that the Observable Datasette client can fetch from to verify that the passed in URL is a valid Datasette (doesnt solve the slow problem, feel free to split this issue into 2)
2. Have a way to turn off table counts when accessing a database's `.json` view, like `?no_count=1` or something
3. Maybe have a timeout on the `table_counts()` function if it takes too long. which is odd, because it seems like it already does that (I think?), I can debug a little more if that's the case

More than happy to debug further, or send a PR if you like one of the proposals above!