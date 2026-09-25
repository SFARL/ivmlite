with cte as (
  select rowid, date, county, state, fips, cases, deaths
  from ny_times_us_counties
),
truncated as (
  select null as _facet, null as facet_name, null as facet_count, rowid, date, county, state, fips, cases, deaths
  from cte order by date desc limit 4
),
state_facet as (
  select 'state' as _facet, state as facet_name, count(*) as facet_count,
  null, null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
fips_facet as (
  select 'fips' as _facet, fips as facet_name, count(*) as facet_count,
  null, null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
county_facet as (
  select 'county' as _facet, county as facet_name, count(*) as facet_count,
  null, null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
total_count as (
  select 'COUNT' as _facet, '' as facet_name, count(*) as facet_count,
  null, null, null, null, null, null, null
  from cte
)
select * from truncated
union all select * from state_facet
union all select * from fips_facet
union all select * from county_facet
union all select * from total_count
