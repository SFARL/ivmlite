from pathlib import Path
import hashlib, json, os, shutil, subprocess
root = Path(__file__).resolve().parent
library = Path(os.environ['IVMLITE_EXTENSION'])
setup = f'''.bail on
.load {library} sqlite3_ivmlite_init
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
'''
queries = []
for index, rhs in enumerate(['@g', "'042'", "42.5", "CAST('042' AS NUMERIC)", "CAST(x'80' AS TEXT)", "x'80'", 'NULL']):
    for op_index, op in enumerate(['=', '>', '>=', '<', '<=']):
        predicate = f'g {op} {rhs}'
        queries.append((f'compare_{index}_{op_index}', f'''SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM roll WHERE {predicate}
            EXCEPT SELECT typeof(g),g,t,s,n FROM expected WHERE {predicate})'''))
        queries.append((f'reverse_{index}_{op_index}', f'''SELECT COUNT(*) FROM (
            SELECT typeof(g),g,t,s,n FROM expected WHERE {predicate}
            EXCEPT SELECT typeof(g),g,t,s,n FROM roll WHERE {predicate})'''))
queries.append(('window', 'SELECT COUNT(*)-1 FROM roll WHERE g=@g AND t>=@lo AND t<@hi'))
checks = '\n'.join(f"SELECT '{name}',({sql});" for name, sql in queries)
transaction = '''
BEGIN;
INSERT INTO facts VALUES(7,42,1,10);
INSERT INTO roll(roll) VALUES('refresh');
SELECT 'updated',(SELECT s-11 FROM roll WHERE g=@g AND t=1);
ROLLBACK;
SELECT 'rollback',(SELECT s-1 FROM roll WHERE g=@g AND t=1);
SELECT 'utf8_count',(SELECT COUNT(*)-4 FROM roll WHERE g<CAST(x'80' AS TEXT));
'''
script = setup + checks + '\n' + transaction
(root / 'smoke.sql').write_text(script)
run = subprocess.run([str(root/'sqlite3'), ':memory:'], input=script, text=True, capture_output=True)
(root/'smoke.stdout').write_text(run.stdout)
(root/'smoke.stderr').write_text(run.stderr)
assert run.returncode == 0, run.stderr
results = [line.split('|') for line in run.stdout.splitlines()]
assert len(results) == len(queries)+3, results
assert all(value == '0' for name, value in results), results
compat = root/'compatibility.sqlite3'
shutil.copyfile(root.parent/'baseline/alpha6.sqlite3', compat)
compat_sql = f'''.bail on
.load {library} sqlite3_ivmlite_init
.mode list
.separator |
SELECT 'reopen_count',(SELECT COUNT(*)-4 FROM roll);
SELECT 'reopen_bound',(SELECT SUM(\"SUM(v)\")-8 FROM roll WHERE g=42);
BEGIN;
INSERT INTO facts VALUES(5,42,1,11);
INSERT INTO roll(roll) VALUES('refresh');
SELECT 'reopen_refresh',(SELECT \"SUM(v)\"-14 FROM roll WHERE g=42 AND t=1);
ROLLBACK;
SELECT 'reopen_rollback',(SELECT \"SUM(v)\"-3 FROM roll WHERE g=42 AND t=1);
'''
(root/'compatibility.sql').write_text(compat_sql)
second = subprocess.run([str(root/'sqlite3'), str(compat)], input=compat_sql, text=True, capture_output=True)
(root/'compatibility.stdout').write_text(second.stdout)
(root/'compatibility.stderr').write_text(second.stderr)
assert second.returncode == 0, second.stderr
compat_results = [line.split('|') for line in second.stdout.splitlines()]
assert len(compat_results) == 4 and all(value=='0' for name,value in compat_results), compat_results
result = {'sqlite': subprocess.check_output([str(root/'sqlite3'),'-version'],text=True).strip(), 'library_sha256': hashlib.sha256(library.read_bytes()).hexdigest(),'smoke_checks':len(results), 'alpha6_reopen_checks':len(compat_results),'failures':0}
(root/'smoke.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
