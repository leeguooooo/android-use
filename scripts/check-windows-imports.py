#!/usr/bin/env python3
"""Reject release binaries that require separately installed MSVC runtime DLLs."""
import pathlib,struct,sys
blob=pathlib.Path(sys.argv[1]).read_bytes()
pe=struct.unpack_from('<I',blob,0x3c)[0]
assert blob[pe:pe+4]==b'PE\0\0'
count=struct.unpack_from('<H',blob,pe+6)[0]
optional=pe+24
assert struct.unpack_from('<H',blob,optional)[0]==0x20b
sections=optional+struct.unpack_from('<H',blob,pe+20)[0]
areas=[]
for i in range(count):
    size,va,raw_size,raw=struct.unpack_from('<IIII',blob,sections+i*40+8)
    areas.append((va,max(size,raw_size),raw))
def offset(rva):
    for va,size,raw in areas:
        if va<=rva<va+size:return raw+rva-va
    raise ValueError('Unmapped import address')
cursor=offset(struct.unpack_from('<I',blob,optional+120)[0])
imports=[]
while any(blob[cursor:cursor+20]):
    name=offset(struct.unpack_from('<I',blob,cursor+12)[0])
    imports.append(blob[name:blob.index(0,name)].decode('ascii').lower())
    cursor+=20
external=[name for name in imports if name.startswith(('vcruntime','msvcp','msvcr'))]
if external:
    print('External MSVC runtime dependency: '+', '.join(external),file=sys.stderr)
    sys.exit(1)
print('Windows imports verified: no external MSVC runtime DLLs')
