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
