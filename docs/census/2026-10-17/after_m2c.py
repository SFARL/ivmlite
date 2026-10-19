#!/usr/bin/env python3
"""Read census snapshots and derive M2c compatibility counts; never edit the repo."""
import hashlib
import json
import sys
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[3]
census = root / 'docs/census/2026-10-17'
supported = {'F3a', 'F3b', 'F3c', 'F3e', 'F10a', 'F10b'}
blockers = {'F13', 'F14', 'F15'}
apps = json.loads((census / 'apps.json').read_text())['items']
eligible = [x for x in apps if x['fit'] != 'not_ivm_shaped'
            and not (set(x['in_place_unlock']) & blockers)]
fits = sorted((x for x in eligible if set(x['in_place_unlock']) <= supported), key=lambda x: x['id'])
excluded = sorted((x for x in apps if x not in eligible), key=lambda x: x['id'])
remaining = sorted((x for x in eligible if x not in fits), key=lambda x: x['id'])
nexmark = json.loads((census / 'nexmark.json').read_text())['items']
def nxfit(bundle):
    return [x['id'] for x in nexmark if x['fit'] != 'out_of_scope'
            and set(x['minimal_unlock']) <= bundle]
bundles = [({'F1'}, 'F1'), ({'F1', 'F4b'}, 'F1 + F4b'),
           ({'F1', 'F4b', 'F6'}, 'F1 + F4b + F6'),
           ({'F1', 'F4b', 'F6', 'F10b'}, 'F1 + F4b + F6 + F10b')]
result = {
    'label': 'Compatibility projection from pinned census, not adoption or newly executed upstream workloads',
    'input_sha256': {name: hashlib.sha256((census / name).read_bytes()).hexdigest()
                     for name in ('apps.json', 'apps.md', 'nexmark.json')},
    'supported_features': sorted(supported),
    'applications': {'total': len(apps), 'eligible_after_non_sql_blockers': len(eligible),
        'after_m2c_fit_count': len(fits),
        'fits': [{'id': x['id'], 'confidence': x['confidence']['level'],
                  'in_place_unlock': x['in_place_unlock'], 'reason': x['confidence']['reason']} for x in fits],
        'still_requires_sql_features': [{'id': x['id'], 'remaining': sorted(set(x['in_place_unlock']) - supported)} for x in remaining],
        'blocked_count': len(excluded), 'blocked_ids': [x['id'] for x in excluded]},
    'nexmark': {'m2c_only_as_written': nxfit(supported),
        'bundles_as_written': [{'bundle': label, 'count': len(nxfit(bundle)), 'queries': nxfit(bundle)} for bundle, label in bundles],
        'after_m2c_plus_F1_F4b_F6_as_written': nxfit(supported | {'F1', 'F4b', 'F6'})}
}
print(json.dumps(result, indent=2))
