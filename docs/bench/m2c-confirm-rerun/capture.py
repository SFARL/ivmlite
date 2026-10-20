"""Re-run the single flagged confirm cell, preserving all original captures."""
from pathlib import Path
import csv,hashlib,json,os,platform,subprocess,time,statistics
main=Path('${MAIN_ROOT}')
out=Path('${EVIDENCE_DIR}');out.mkdir(exist_ok=False)
source=main/'docs/bench/m1b-phase6.csv'
with source.open(newline='')as stream:
 reader=csv.DictReader(stream);header=reader.fieldnames
 rows=[r for r in reader if tuple(int(r[k])for k in ['views','base_rows','batch_size','group_cardinality'])==(1,10000,100,1000)]
if len(rows)!=4 or len({r['engine']for r in rows})!=4:raise ValueError('expected all four original engine rows')
fixture=out/'selection.csv'
with fixture.open('w',newline='')as stream:
 writer=csv.DictWriter(stream,fieldnames=header);writer.writeheader();writer.writerows(rows)
records=[]
for label,sha in [('before','b647bcba9fa5f8eb583b162f87f7dfd79a10f7fa'),('after','84e4c07c2ebfc9db8b6bafb57378627414df5524')]:
 root=Path('${FROZEN_ROOT_PREFIX}'+label+'/ivm')
 actual=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
 if actual!=sha or subprocess.check_output(['git','status','--porcelain'],cwd=root,text=True).strip():raise ValueError('frozen source must be clean')
 binary=root/'target/release/ivmlite-bench';library=root/'crates/ivmlite-sqlite/target/release/libivmlite_sqlite.dylib'
 command=[str(binary),'confirm','--from',str(fixture),'--workload',str(root/'workloads/m0-baseline.toml'),'--extension',str(library)]
 record={'label':label,'source_commit':sha,'harness_commit':actual,'command':command,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'library_sha256':hashlib.sha256(library.read_bytes()).hexdigest(),'selection_source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'selection_sha256':hashlib.sha256(fixture.read_bytes()).hexdigest(),'started_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'load_start':os.getloadavg(),'platform':platform.platform()}
 start=time.monotonic()
 with (out/(label+'.csv')).open('x')as stdout,(out/(label+'.stderr.log')).open('x')as stderr:
  result=subprocess.run(['caffeinate','-i',*command],cwd=root,stdout=stdout,stderr=stderr)
 record.update(returncode=result.returncode,wall_seconds=time.monotonic()-start,load_end=os.getloadavg())
 records.append(record);(out/'runs.json').write_text(json.dumps(records,indent=2)+'\n')
 if result.returncode:raise SystemExit(result.returncode)
summary=[]
for label in ['before','after']:
 with (out/(label+'.csv')).open(newline='')as stream:data=list(csv.DictReader(stream))
 if len(data)!=20:raise ValueError(f'{label}: wanted20rows')
 for engine in sorted({r['engine']for r in data}):
  part=[r for r in data if r['engine']==engine]
  if {int(r['repeat'])for r in part}!={0,1,2,3,4}:raise ValueError('repeat inventory differs')
  for metric in ['maintain_ms','apply_plus_maintain_ms']:
   values=[float(r[metric])for r in part]
   summary.append({'label':label,'engine':engine,'metric':metric,'median':statistics.median(values),'min':min(values),'max':max(values)})
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
for engine in sorted({r['engine']for r in summary}):
 for metric in ['maintain_ms','apply_plus_maintain_ms']:
  before=next(r for r in summary if r['label']=='before'and r['engine']==engine and r['metric']==metric)
  after=next(r for r in summary if r['label']=='after'and r['engine']==engine and r['metric']==metric)
  print(json.dumps({'engine':engine,'metric':metric,'before':before,'after':after,'after_before':after['median']/before['median']if before['median']else None}))
