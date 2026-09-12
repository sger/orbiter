#!/usr/bin/env python3
"""Generate license metadata for a target. Requires resolved Cargo/npm dependencies."""
import json
import subprocess
import sys
from pathlib import Path

target = sys.argv[1] if len(sys.argv) > 1 else 'aarch64-apple-darwin'
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target]))
resolved = {node['id'] for node in metadata['resolve']['nodes']}
rows = [{'ecosystem': 'cargo', 'name': p['name'], 'version': p['version'], 'license': p['license'], 'repository': p['repository']} for p in metadata['packages'] if p['id'] in resolved and p['source']]
lock = json.loads(Path('package-lock.json').read_text())
for location, package in lock['packages'].items():
    if not location:
        continue
    manifest = Path(location) / 'package.json'
    local = json.loads(manifest.read_text()) if manifest.exists() else {}
    rows.append({'ecosystem': 'npm', 'name': location.split('node_modules/')[-1], 'version': package['version'], 'license': package.get('license', local.get('license')), 'development': package.get('dev', False)})
result = {'target': target, 'scope': 'Resolved build/test metadata; not a redistribution notice bundle or legal approval.', 'packages': sorted(rows, key=lambda r: (r['ecosystem'], r['name'], r['version']))}
Path(f'docs/dependencies-{target}.json').write_text(json.dumps(result, indent=2) + '\n')
