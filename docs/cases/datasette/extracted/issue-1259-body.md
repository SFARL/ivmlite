https://www.sqlite.org/changes.html#version_3_35_0

> Add support for the [MATERIALIZED](https://www.sqlite.org/lang_with.html#mathint) and [NOT MATERIALIZED](https://www.sqlite.org/lang_with.html#mathint) hints when specifying [common table expressions](https://www.sqlite.org/lang_with.html). The default behavior was formerly NOT MATERIALIZED, but is now changed to MATERIALIZED for CTEs that are used more than once.

If a CTE creates a table that is used multiple time in that query, SQLite will now default to creating a materialized table for the duration of that query.

This could be a big performance boost when applying faceting multiple times against the same query. Consider this example query:
```sql
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
```
https://global-power-plants.datasettes.com/global-power-plants?sql=WITH+data+as+%28%0D%0A++select%0D%0A++++*%0D%0A++from%0D%0A++++%5Bglobal-power-plants%5D%0D%0A%29%2C%0D%0Acountry_long+as+%28select+%0D%0A++%27country_long%27+as+col%2C+country_long+as+value%2C+count%28*%29+as+c+from+data+group+by+country_long%0D%0A++order+by+c+desc+limit+10%0D%0A%29%2C%0D%0Aprimary_fuel+as+%28%0D%0Aselect%0D%0A++%27primary_fuel%27+as+col%2C+primary_fuel+as+value%2C+count%28*%29+as+c+from+data+group+by+primary_fuel%0D%0A++order+by+c+desc+limit+10%0D%0A%29%0D%0Aselect+*+from+primary_fuel+union+select+*+from+country_long+order+by+col%2C+c+desc

Outputs:

col | value | c
-- | -- | --
country_long | United States of America | 8688
country_long | China | 4235
country_long | United Kingdom | 2603
country_long | Brazil | 2360
country_long | France | 2155
country_long | India | 1590
country_long | Germany | 1309
country_long | Canada | 1159
country_long | Spain | 829
country_long | Russia | 545
primary_fuel | Solar | 9662
primary_fuel | Hydro | 7155
primary_fuel | Wind | 5188
primary_fuel | Gas | 3922
primary_fuel | Coal | 2390
primary_fuel | Oil | 2290
primary_fuel | Biomass | 1396
primary_fuel | Waste | 1087
primary_fuel | Nuclear | 198
primary_fuel | Geothermal | 189
