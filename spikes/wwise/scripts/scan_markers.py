#!/usr/bin/env python3
"""Count streamed .wem files with RIFF "cue " chunks (Wwise markers) in .pck packages.

Usage: python3 -I scan_markers.py <package.pck>... (read-only)."""
import os
import struct
import sys
def lut(b, off, wide):
    n, = struct.unpack_from('<I', b, off); off += 4; out=[]
    for i in range(n):
        if wide:
            fid, bs, sz, st, lang = struct.unpack_from('<QIIII', b, off); off += 24
        else:
            fid, bs, sz, st, lang = struct.unpack_from('<IIIII', b, off); off += 20
        out.append((fid, bs*st, sz))
    return out
tot=0; hits=[]
for path in sys.argv[1:]:
    f = open(path,'rb'); head = f.read(8); hs, = struct.unpack_from('<I', head, 4)
    h = head + f.read(hs)
    sizes = struct.unpack_from('<IIII', h, 12)
    off = 28 + sizes[0]; banks = lut(h, off, False); off += sizes[1]; stm = lut(h, off, False)
    for fid, o, sz in stm:
        f.seek(o); d = f.read(min(sz, 4096)); tot += 1
        # walk RIFF chunks
        if d[:4] != b'RIFF': continue
        p = 12
        while p + 8 <= len(d):
            cid = d[p:p+4]; cl, = struct.unpack_from('<I', d, p+4)
            if cid == b'cue ':
                n, = struct.unpack_from('<I', d, p+8); hits.append((os.path.basename(path), fid, n)); break
            if cid == b'data': break
            p += 8 + cl + (cl & 1)
print('scanned', tot, 'streamed files; with cue markers:', len(hits))
for h in hits[:15]: print(h)
