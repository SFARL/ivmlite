WITH data as (
  select
    *
  from
    [global-power-plants]
),
country_long as (select 
  'country_long' as col, country_long as value, count(*) as c from data group by country_long
  order by c desc limit 10
),
primary_fuel as (
select
  'primary_fuel' as col, primary_fuel as value, count(*) as c from data group by primary_fuel
  order by c desc limit 10
)
select * from primary_fuel union select * from country_long order by col, c desc
