#!/usr/bin/env python3
"""Publish the existing Task 13 capture with path normalization; never run a benchmark."""
import argparse
import hashlib
import json
from pathlib import Path


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True,
                        help='Directory containing the original runs.jsonl and raw captures')
    args = parser.parse_args()
    destination = Path(__file__).resolve().parent
    repo = destination.parents[2]
    records = [json.loads(line) for line in (args.source / 'runs.jsonl').read_text().splitlines()]
    roots = {
        next(row['cwd'] for row in records if row['name'] == 'ablation'): '${AFTER_HARNESS_TREE}',
        next(row['cwd'] for row in records if row['name'] == 'confirm-before'): '${BEFORE_TREE}',
        next(row['cwd'] for row in records if row['name'] == 'confirm-after'): '${AFTER_EXTENSION_TREE}',
        str(args.source.resolve()): '${CAPTURE_ROOT}',
    }

    def normalize(data):
        text = data.decode('utf-8')
        for source, replacement in sorted(roots.items(), key=lambda item: -len(item[0])):
            text = text.replace(source, replacement)
        if '/Users/' in text:
            raise ValueError('Unrecognized private absolute path in capture')
        return text.encode('utf-8')

    names = ['machine.json', 'runs.jsonl', 'raw-capture-provenance.json',
             'application-progress-2.log', 'fluxflow-before.txt', 'fluxflow-after.txt',
             'kener-before.txt', 'kener-after.txt']
    for name in names[4:]:
        if '# sqlite: 3.53.2 ' not in (args.source / name).read_text():
            raise ValueError(f'Expected actual bundled SQLite version missing: {name}')
    artifacts = []
    for name in names:
        original = (args.source / name).read_bytes()
        normalized = normalize(original)
        changes = ['Replace recorded local root paths with documented placeholders.']
        if name == 'machine.json':
            machine = json.loads(normalized)
            machine['sqlite_version'] = '3.53.2'
            machine['sqlite_version_evidence'] = 'Actual # sqlite provenance lines in all four raw demo captures and scoped build CSVs.'
            normalized = (json.dumps(machine, indent=2) + '\n').encode('utf-8')
            changes.append('Resolve original pending SQLite field from actual raw outputs; retain original file hash.')
        (destination / name).write_bytes(normalized)
        artifacts.append({'source_name': name, 'published_name': name,
                          'original_sha256': sha256(original),
                          'published_sha256': sha256(normalized), 'normalization': changes})
    csv_paths = sorted((repo / 'docs/bench').rglob('*.csv'))
    csv_paths = [path for path in csv_paths
                 if path.name.startswith('m2c-') or 'm2c-confirm-rerun' in path.parts]
    csv_hashes = {str(path.relative_to(repo)): sha256(path.read_bytes()) for path in csv_paths}
    combined_hashes = {str(path.relative_to(repo)): sha256(path.read_bytes())
                       for path in sorted((repo / 'docs/demos/results').glob('2026-10-20-*-m2c.txt'))}
    manifest = {'description': 'Original capture identities and published normalized byte identities; CSV bytes are unchanged.',
                'artifacts': artifacts, 'csv_sha256': csv_hashes,
                'combined_demo_capture_sha256': combined_hashes}
    (destination / 'hashes.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    main()
