"""Audit raw native rounds and derive repeat-median summaries; no native work."""
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys

ROOT = Path(sys.argv[1]).resolve() if len(sys.argv)>1 else Path(__file__).resolve().parent

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def audit(data, repeats, size, mode):
    assert data['metadata']['base_rows']==size
    assert len(data['repeats'])==repeats
    frame=[]
    for index,repeat in enumerate(data['repeats']):
        assert repeat['repeat']==index and repeat['oracle_checks_passed'] is True
        assert repeat['initial_groups']==(550000 if size==4100000 else 13414)
        rounds=repeat['rounds']; assert len(rounds)==11
        states=[]
        for n,round in enumerate(rounds):
            assert round['round']==n and round['repeat']==index and round['warmup']==(n==0)
            names=['full','monitor','window']+(['nonleading'] if mode=='work' else [])
            assert round['read_order']==names[n%len(names):]+names[:n%len(names)]
            reads={r['shape']:r for r in round['reads']}; assert set(reads)==set(names)
            assert reads['full']['outer_rows']>reads['monitor']['outer_rows']>=reads['window']['outer_rows']
            if mode=='latency':
                times=round['timings_ms']; assert all(math.isfinite(v) and v>=0 for v in times.values())
                for name in names:
                    total=times['write']+times['refresh']+times[name]+times['commit']
                    assert math.isclose(times['write_refresh_read_commit_'+name],total,abs_tol=1e-8)
                    assert math.isclose(times['end_to_end_'+name],total+times['begin'],abs_tol=1e-8)
                assert all('work' not in r for r in reads.values())
            else:
                assert 'timings_ms' not in round and 'measured_round_summary_ms' not in repeat and 'bootstrap_ms' not in repeat
                for read in reads.values():
                    w=read['work']; assert w['separate_pass_outer_rows']==read['outer_rows']
                    assert w['nested'] and all(v['fullscan_steps']>=0 and v['vm_steps']>0 and v['rows_returned']>=0 for v in w['nested'])
            states.append({name:reads[name]['outer_rows'] for name in ['full','monitor','window']})
        if mode=='latency':
            for field,summary in repeat['measured_round_summary_ms'].items():
                values=[r['timings_ms'][field] for r in rounds[1:]]
                expected={'min':min(values),'median':statistics.median(values),'max':max(values)}
                for k,v in expected.items(): assert math.isclose(v,summary[k],abs_tol=1e-9),(field,k)
        frame.append({'groups':repeat['initial_groups'],'allocations':{k:v for k,v in repeat.items() if k.startswith('allocation_')},'states':states})
    assert all(frame[0]==s for s in frame), 'Fresh repeats have inconsistent states/allocations'
    return frame[0]

def summarize(data):
    summaries=data['repeats']
    return {key:{'median':statistics.median(values),'min':min(values),'max':max(values),'repeat_medians':values} for key in summaries[0]['measured_round_summary_ms'] for values in [[r['measured_round_summary_ms'][key]['median'] for r in summaries]]}

def work_summary(data):
    names=['full','monitor','window','nonleading'];output={}
    for shape in names:
        metrics=[]
        for round in data['repeats'][0]['rounds'][1:]:
            read=next(r for r in round['reads'] if r['shape']==shape);w=read['work']
            metrics.append({'outer_rows':read['outer_rows'],'nested_candidates':sum(x['rows_returned'] for x in w['nested']),'nested_fullscan_steps':sum(x['fullscan_steps'] for x in w['nested']),'nested_vm_steps':sum(x['vm_steps'] for x in w['nested']),'whole_query_progress_callbacks':w['whole_query_progress_callbacks_separate_pass']})
        output[shape]={key:{'min':min(v),'median':statistics.median(v),'max':max(v)} for key in metrics[0] for v in [[x[key] for x in metrics]]}
    return output

def main():
    loaded={}
    for phase,size,repeats in [('quick',100000,1),('source',4100000,5)]:
        frames=[]
        for mode in ['latency','work']:
            for label in ['before','after']:
                name=f'{phase}-{label}-{mode}';data=json.loads((ROOT/(name+'.json')).read_text());loaded[name]=data
                frames.append(audit(data,repeats if mode=='latency' else 1,size,mode))
        assert all(frames[0]==f for f in frames), f'{phase}: old/new latency/work state or page allocation differs'
    summary={'source_latency_ms':{label:summarize(loaded[f'source-{label}-latency']) for label in ['before','after']},'source_work':{label:work_summary(loaded[f'source-{label}-work']) for label in ['before','after']},'input_sha256':{name+'.json':sha(ROOT/(name+'.json')) for name in loaded}}
    (ROOT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print('PASS: raw round, warmup, total, state, allocation and cross-mode consistency audits')
    for metric,old in summary['source_latency_ms']['before'].items():
        new=summary['source_latency_ms']['after'][metric]
        print(f'{metric}: {old["median"]:.6f} [{old["min"]:.6f},{old["max"]:.6f}] -> {new["median"]:.6f} [{new["min"]:.6f},{new["max"]:.6f}] ms; ratio {old["median"]/new["median"]:.3f}x')

if __name__=='__main__':
    main()
