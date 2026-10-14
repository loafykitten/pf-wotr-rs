"""Extract Metal (MSL) subprograms from Unity 2020.3 Shader objects.

usage: extract_msl.py OUTDIR FILE [FILE...]   (env SHADERS=name1,name2 to filter; DUMP_ALL=1 writes every blob)
Writes OUTDIR/<shader>/index.tsv (pass, lightmode, stage, blob, gpu_type, keywords, msl_bytes)
and OUTDIR/<shader>/blob<N>.metal for each blob (all if DUMP_ALL, otherwise first variant per pass+stage).
"""
import os, re, struct, sys, UnityPy
from UnityPy.helpers import CompressionHelper

out = sys.argv[1]
want = set(filter(None, os.environ.get("SHADERS", "").split(",")))
dump_all = os.environ.get("DUMP_ALL") == "1"
summary = []

def read_str(b, p):
    n = struct.unpack_from("<i", b, p)[0]; p += 4
    s = b[p:p+n].decode("latin1"); p += n
    p = (p + 3) & ~3
    return s, p

def parse_sub(sp):
    # version, gpu program type, 4 stats ints? then keywords and local keywords, then code
    p = 0
    ver, gtype = struct.unpack_from("<2i", sp, p); p += 8
    p += 16  # statsALU, statsTEX, statsFlow, statsTempRegister
    kws = []
    for _ in range(2):  # global then local keywords
        n = struct.unpack_from("<i", sp, p)[0]; p += 4
        for _ in range(n):
            s, p = read_str(sp, p); kws.append(s)
    clen = struct.unpack_from("<i", sp, p)[0]; p += 4
    code = sp[p:p+clen]
    j = code.find(b"#include")
    msl = code[j:].rstrip(b"\0") if j >= 0 else b""
    return ver, gtype, kws, msl

for path in sys.argv[2:]:
    env = UnityPy.load(path)
    for obj in env.objects:
        if obj.type.name != "Shader": continue
        sh = obj.read()
        name = sh.m_ParsedForm.m_Name
        if want and name not in want: continue
        if 14 not in sh.platforms: continue
        pi = sh.platforms.index(14)
        blob = bytes(sh.compressedBlob)
        offs, cl, dl = sh.offsets[pi], sh.compressedLengths[pi], sh.decompressedLengths[pi]
        if not isinstance(offs, list): offs, cl, dl = [offs], [cl], [dl]
        segs = [CompressionHelper.decompress_lz4(blob[o:o+c], d) for o, c, d in zip(offs, cl, dl)]
        n = struct.unpack_from("<i", segs[0], 0)[0]
        ents = [struct.unpack_from("<3i", segs[0], 4 + 12*i) for i in range(n)]
        d = obj.read_typetree()["m_ParsedForm"]
        sdir = os.path.join(out, re.sub(r"[^A-Za-z0-9_.-]+", "_", name))
        os.makedirs(sdir, exist_ok=True)
        rows, total_msl, dumped = [], 0, set()
        for ssi, ss in enumerate(d["m_SubShaders"]):
            for pas in ss["m_Passes"]:
                pname = pas["m_State"]["m_Name"]
                tags = dict(pas["m_State"]["m_Tags"]["tags"])
                lm = tags.get("LIGHTMODE", "")
                for stage in ("progVertex", "progFragment", "progGeometry", "progHull", "progDomain"):
                    for k, sub in enumerate(pas[stage]["m_SubPrograms"]):
                        bi = sub["m_BlobIndex"]
                        o, l, s = ents[bi]
                        ver, gtype, kws, msl = parse_sub(segs[s][o:o+l])
                        total_msl += len(msl)
                        rows.append((ssi, pname, lm, stage, bi, gtype, " ".join(kws), len(msl)))
                        key = (ssi, pname, stage)
                        if dump_all or key not in dumped:
                            dumped.add(key)
                            open(os.path.join(sdir, f"blob{bi}.metal"), "wb").write(msl)
        with open(os.path.join(sdir, "index.tsv"), "w") as f:
            f.write("subshader\tpass\tlightmode\tstage\tblob\tgpu_type\tkeywords\tmsl_bytes\n")
            for r in rows: f.write("\t".join(map(str, r)) + "\n")
        summary.append((name, n, len(rows), total_msl))
        print(f"{name}\tblobs={n}\tsubprograms={len(rows)}\tmsl_MB={total_msl/1e6:.1f}", flush=True)
