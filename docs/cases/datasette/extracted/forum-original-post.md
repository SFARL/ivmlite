
My application [Datasette](https://datasette.io) has a feature called "faceting", where I display a table with several group-by-count queries to allow people to filter the results.

Here's an example page: <https://covid-19.datasettes.com/covid/ny_times_us_counties?state=Kentucky>

That page executes the following four queries:

- `select rowid, date, county, state, fips, cases, deaths from ny_times_us_counties where state = 'Kentucky' order by date desc limit 101` - 78ms
- `select count(*) from ny_times_us_counties where state = 'Kentucky'` - 7ms
- `select state as value, count(*) as count from ( select rowid, date, county, state, fips, cases, deaths from ny_times_us_counties where state = 'Kentucky'  )  where state is not null   group by state order by count desc, value limit 31` - 12ms
- `select county as value, count(*) as count from ( select rowid, date, county, state, fips, cases, deaths from ny_times_us_counties where state = 'Kentucky'  )  where county is not null   group by county order by count desc, value limit 31` - 50ms
- `select fips as value, count(*) as count from ( select rowid, date, county, state, fips, cases, deaths from ny_times_us_counties where state = 'Kentucky'  )  where fips  is not null   group by fips order by count desc, value limit 31` - 52ms

Total for all 5 queries: 199ms

I'm always looking for ways to speed up these kinds of queries, since my application runs them a lot (suggestions very welcome).

Today I had a bright idea: what if I combined all of the above into a single query using a CTE for the initial selection? Could this give me a speed boost by helping SQLite avoid creating the same filtered table multiple times as part of each query?

Here's the SQL query I came up with:

```sql
with filtered as (
  select rowid, date, county, state, fips, cases, deaths
  from ny_times_us_counties where state = 'Kentucky'
),
rows as (
  select null as facet, null as favet_name, null as facet_value,
  rowid, date, county, state, fips, cases, deaths
  from filtered order by date desc limit 101
),
count as (
  select 'COUNT' as facet, null as facet_name, count(*) as facet_value,
  null, null, null, null, null, null, null
  from filtered
),
facet_state as (
  select 'state' as facet, state as facet_name, count(*) as facet_value,
  null, null, null, null, null, null, null
  from filtered group by facet_name order by facet_value desc limit 31
),
facet_county as (
  select 'county' as facet, county as facet_name, count(*) as facet_value,
  null, null, null, null, null, null, null
  from filtered group by facet_name order by facet_value desc limit 31
),
facet_fips as (
  select 'fips' as facet, fips as facet_name, count(*) as facet_value,
  null, null, null, null, null, null, null
  from filtered group by facet_name order by facet_value desc limit 31
)
select * from rows
union all
select * from count
union all
select * from facet_state
union all
select * from facet_county
union all
select * from facet_fips
```
[You can try that query here](https://covid-19.datasettes.com/covid?sql=with+filtered+as+%28%0D%0A++select+rowid%2C+date%2C+county%2C+state%2C+fips%2C+cases%2C+deaths%0D%0A++from+ny_times_us_counties+where+state+%3D+%27Kentucky%27%0D%0A%29%2C%0D%0Arows+as+%28%0D%0A++select+null+as+facet%2C+null+as+favet_name%2C+null+as+facet_value%2C%0D%0A++rowid%2C+date%2C+county%2C+state%2C+fips%2C+cases%2C+deaths%0D%0A++from+filtered+order+by+date+desc+limit+101%0D%0A%29%2C%0D%0Acount+as+%28%0D%0A++select+%27COUNT%27+as+facet%2C+null+as+facet_name%2C+count%28*%29+as+facet_value%2C%0D%0A++null%2C+null%2C+null%2C+null%2C+null%2C+null%2C+null%0D%0A++from+filtered%0D%0A%29%2C%0D%0Afacet_state+as+%28%0D%0A++select+%27state%27+as+facet%2C+state+as+facet_name%2C+count%28*%29+as+facet_value%2C%0D%0A++null%2C+null%2C+null%2C+null%2C+null%2C+null%2C+null%0D%0A++from+filtered+group+by+facet_name+order+by+facet_value+desc+limit+31%0D%0A%29%2C%0D%0Afacet_county+as+%28%0D%0A++select+%27county%27+as+facet%2C+county+as+facet_name%2C+count%28*%29+as+facet_value%2C%0D%0A++null%2C+null%2C+null%2C+null%2C+null%2C+null%2C+null%0D%0A++from+filtered+group+by+facet_name+order+by+facet_value+desc+limit+31%0D%0A%29%2C%0D%0Afacet_fips+as+%28%0D%0A++select+%27fips%27+as+facet%2C+fips+as+facet_name%2C+count%28*%29+as+facet_value%2C%0D%0A++null%2C+null%2C+null%2C+null%2C+null%2C+null%2C+null%0D%0A++from+filtered+group+by+facet_name+order+by+facet_value+desc+limit+31%0D%0A%29%0D%0Aselect+*+from+rows%0D%0Aunion+all%0D%0Aselect+*+from+count%0D%0Aunion+all%0D%0Aselect+*+from+facet_state%0D%0Aunion+all%0D%0Aselect+*+from+facet_county%0D%0Aunion+all%0D%0Aselect+*+from+facet_fips) - the clever (I thought) idea here is to use `union all` to execute all five queries in one do, and hopefully have the query planner take advantage of and reuse the CTE.

This query takes 200ms - and in some cases I've seen it take significantly longer than the 5 queries added up!

I was expecting my clever huge CTE/Union query to beat the separate queries, and instead it's consistently losing to them.

I'd love to understand why, mainly out of curiosity but also to help
me figure out if there's an optimization here that I'm missing.
