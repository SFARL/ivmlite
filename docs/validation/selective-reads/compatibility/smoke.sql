.bail on
.load ./ivmlite-extension sqlite3_ivmlite_init
.mode list
.separator |
CREATE TABLE facts(id INTEGER PRIMARY KEY,g ANY,t INTEGER,v INTEGER) STRICT;
INSERT INTO facts VALUES(1,42,1,1),(2,'042',2,2),(3,42.5,3,3),(4,'x',4,4),(5,NULL,5,5),(6,x'80',6,6);
CREATE VIRTUAL TABLE roll USING ivm('SELECT g,t,SUM(v) AS s,COUNT(*) AS n FROM facts GROUP BY g,t');
CREATE TABLE expected(g,t,s,n);
INSERT INTO expected SELECT g,t,SUM(v),COUNT(*) FROM facts GROUP BY g,t;
.parameter init
.parameter set @g 42
.parameter set @lo 0
.parameter set @hi 4
SELECT 'compare_0_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = @g));
SELECT 'reverse_0_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = @g));
SELECT 'compare_0_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > @g));
SELECT 'reverse_0_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > @g));
SELECT 'compare_0_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= @g));
SELECT 'reverse_0_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= @g));
SELECT 'compare_0_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < @g));
SELECT 'reverse_0_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < @g));
SELECT 'compare_0_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= @g));
SELECT 'reverse_0_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= @g
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= @g));
SELECT 'compare_1_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = '042'));
SELECT 'reverse_1_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = '042'));
SELECT 'compare_1_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > '042'));
SELECT 'reverse_1_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > '042'));
SELECT 'compare_1_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= '042'));
SELECT 'reverse_1_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= '042'));
SELECT 'compare_1_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < '042'));
SELECT 'reverse_1_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < '042'));
SELECT 'compare_1_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= '042'));
SELECT 'reverse_1_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= '042'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= '042'));
SELECT 'compare_2_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = 42.5));
SELECT 'reverse_2_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = 42.5));
SELECT 'compare_2_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > 42.5));
SELECT 'reverse_2_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > 42.5));
SELECT 'compare_2_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= 42.5));
SELECT 'reverse_2_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= 42.5));
SELECT 'compare_2_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < 42.5));
SELECT 'reverse_2_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < 42.5));
SELECT 'compare_2_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= 42.5));
SELECT 'reverse_2_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= 42.5
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= 42.5));
SELECT 'compare_3_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = CAST('042' AS NUMERIC)));
SELECT 'reverse_3_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = CAST('042' AS NUMERIC)));
SELECT 'compare_3_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > CAST('042' AS NUMERIC)));
SELECT 'reverse_3_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > CAST('042' AS NUMERIC)));
SELECT 'compare_3_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= CAST('042' AS NUMERIC)));
SELECT 'reverse_3_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= CAST('042' AS NUMERIC)));
SELECT 'compare_3_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < CAST('042' AS NUMERIC)));
SELECT 'reverse_3_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < CAST('042' AS NUMERIC)));
SELECT 'compare_3_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= CAST('042' AS NUMERIC)));
SELECT 'reverse_3_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= CAST('042' AS NUMERIC)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= CAST('042' AS NUMERIC)));
SELECT 'compare_4_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = CAST(x'80' AS TEXT)));
SELECT 'reverse_4_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = CAST(x'80' AS TEXT)));
SELECT 'compare_4_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > CAST(x'80' AS TEXT)));
SELECT 'reverse_4_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > CAST(x'80' AS TEXT)));
SELECT 'compare_4_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= CAST(x'80' AS TEXT)));
SELECT 'reverse_4_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= CAST(x'80' AS TEXT)));
SELECT 'compare_4_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < CAST(x'80' AS TEXT)));
SELECT 'reverse_4_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < CAST(x'80' AS TEXT)));
SELECT 'compare_4_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= CAST(x'80' AS TEXT)));
SELECT 'reverse_4_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= CAST(x'80' AS TEXT)
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= CAST(x'80' AS TEXT)));
SELECT 'compare_5_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = x'80'));
SELECT 'reverse_5_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = x'80'));
SELECT 'compare_5_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > x'80'));
SELECT 'reverse_5_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > x'80'));
SELECT 'compare_5_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= x'80'));
SELECT 'reverse_5_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= x'80'));
SELECT 'compare_5_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < x'80'));
SELECT 'reverse_5_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < x'80'));
SELECT 'compare_5_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= x'80'));
SELECT 'reverse_5_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= x'80'
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= x'80'));
SELECT 'compare_6_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g = NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g = NULL));
SELECT 'reverse_6_0',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g = NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g = NULL));
SELECT 'compare_6_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g > NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g > NULL));
SELECT 'reverse_6_1',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g > NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g > NULL));
SELECT 'compare_6_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g >= NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g >= NULL));
SELECT 'reverse_6_2',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g >= NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g >= NULL));
SELECT 'compare_6_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g < NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g < NULL));
SELECT 'reverse_6_3',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g < NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g < NULL));
SELECT 'compare_6_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE g <= NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE g <= NULL));
SELECT 'reverse_6_4',(SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE g <= NULL
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE g <= NULL));
SELECT 'window',(SELECT COUNT(*)-1 FROM roll WHERE g=@g AND t>=@lo AND t<@hi);

BEGIN;
INSERT INTO facts VALUES(7,42,1,10);
INSERT INTO roll(roll) VALUES('refresh');
SELECT 'updated',(SELECT s-11 FROM roll WHERE g=@g AND t=1);
ROLLBACK;
SELECT 'rollback',(SELECT s-1 FROM roll WHERE g=@g AND t=1);
SELECT 'utf8_count',(SELECT COUNT(*)-4 FROM roll WHERE g<CAST(x'80' AS TEXT));
