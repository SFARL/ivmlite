Posting my final tests -
I conclude the following:

[FULL DATA SHEET.](https://hastebin.skyra.pw/eyoxogamaz.org) < **CLICK HERE**

# Conclusions

1. Materialized View wins out clearly when Join Complexity is the criteria.

2. Because of the data structure - joins between `nodes_view` and `files` table is slower than join between `nodes` and `files` table ever so slightly. 

Further we get the result that querying for the intersection of  attributes available in the `nodes` and `nodes_view` take the same amount of time.

The exact rationale as to why there is a slight degradation when joining with `files` table from the `nodes_view` compared to `nodes` is not clear to me.
 
```
select ... from sqlite_stat1;
---------------------------------------------------------------
TABLE | INDEX | ROW SIZE | AVERAGE ENTRIES PER INDEX KEY

files |sqlite_autoindex_files_1 | 10000  1

nodes |sqlite_autoindex_nodes_1 | 20000 1

nodes_view |nodes_view_file | 20000 2

nodes_view | sqlite_autoindex_nodes_view_1| 20000 1
```

Some Quick Statistic:


```
(Control vs Experiment)
+ Query for entries in node-list: 0.6 sec vs 0.2 sec (nodes+files+tags+refs+aliases)
+ Query for * in nodes;: 0.1 seconds
- Query for ... in nodes join files;: 0.16 vs 0.20 seconds
```

So to conclude, 

1. Formulate query from specialized tables for table join < 2 
.: for simple query bw `nodes` & `files`

2. Utilize Materialized Views when table join >=2 
.: for complex query between `nodes`, `files`, `tags`, `refs` & `aliases`

## Also Note:

ALL RESULTS ARE BEST AVERAGES 
- LATENCY GOES UP 4x when doing first query. => 0.2 = 0.8 secs (low 1%), etc
Why Choose best averages? Because they are consistent to compare relative efficiency - when latency goes up - it goes up by equal factors for all classes. These results only show relative efficiency - not absolute time which users should expect to get. Actual times may fluctuate wildly. But they fluctuate by similar factors.