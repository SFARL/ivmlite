with cte as (
  select rowid, country, country_long, name, owner, primary_fuel
  from [global-power-plants]
),
truncated as (
  select null as _facet, null as facet_name, null as facet_count, rowid, country, country_long, name, owner, primary_fuel
  from cte order by rowid limit 4
),
country_long_facet as (
  select 'country_long' as _facet, country_long as facet_name, count(*) as facet_count,
  null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
owner_facet as (
  select 'owner' as _facet, owner as facet_name, count(*) as facet_count,
  null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
primary_fuel_facet as (
  select 'primary_fuel' as _facet, primary_fuel as facet_name, count(*) as facet_count,
  null, null, null, null, null, null
  from cte group by facet_name order by facet_count desc limit 3
),
total_count as (
  select 'COUNT' as _facet, '' as facet_name, count(*) as facet_count,
  null, null, null, null, null, null
  from cte
)
select * from truncated
union all select * from country_long_facet
union all select * from owner_facet
union all select * from primary_fuel_facet
union all select * from total_count
