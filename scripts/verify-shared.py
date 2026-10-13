#!/usr/bin/env python3
"""Check that the shared iphone-use core matches its recorded source snapshot."""
import hashlib,json,pathlib,sys
root=pathlib.Path(__file__).resolve().parents[1]
manifest=json.loads((root/'shared/manifest.json').read_text())
actual={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for name in ['core','control'] for p in (root/'shared'/name).rglob('*') if p.is_file()}
if actual != manifest['sha256']:
    print('Shared core differs from its source manifest',file=sys.stderr);sys.exit(1)
print('Shared core hashes verified')
