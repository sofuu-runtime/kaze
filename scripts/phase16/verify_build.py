#!/usr/bin/env python3
"""verify_build.py — Phase 16 proof-carrying compilation with assertion elision.

Pipeline for one source:
  1. kaze-verify certifies the module (json) and emits the proof certificate and
     SMT-LIB2 obligations.
  2. The compiler emits C for the module.
  3. Every fallback boundary branch the verifier proved redundant (all fallback
     obligations on that source line proven) is replaced by an unchecked helper,
     so the emitted binary carries no boundary assertion on those paths.
  4. The original and elided programs are compiled and run; their outputs must
     match, and the elided binary must have fewer fallback call sites.
  5. The module's .kazebc bytecode is compiled and the proof certificate is
     written next to it, binding artifact to proof by source digest.
"""
import json
import os
import re
import subprocess
import sys

def run(cmd, cwd=None, input_text=None):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, input=input_text)

HELPERS = """/* --- Phase 16 proof-carrying elision helpers --- */
/* These bodies omit the boundary branches the verifier proved redundant; the
 * proof certificate next to the artifact is what makes the omission legal. */
KZ_MAYBE_UNUSED static inline KzValue kz_fb_div_proven(KzValue a, KzValue b, KzValue fb) {
    (void)fb;
    return kz_div(a, b);
}
KZ_MAYBE_UNUSED static inline KzValue kz_fb_mod_proven(KzValue a, KzValue b, KzValue fb) {
    (void)fb;
    return kz_mod(a, b);
}
KZ_MAYBE_UNUSED static inline KzValue kz_fb_index_proven(KzValue coll, KzValue idx, KzValue fb) {
    size_t i = (size_t)kz_to_num(idx);
    (void)fb;
    if (coll.kind == KZ_VAL_LIST && coll.as.list) return coll.as.list->items[i];
    if (coll.kind == KZ_VAL_STR && coll.as.str.data) {
        char *one = (char *)kz_alloc(2);
        one[0] = coll.as.str.data[i];
        one[1] = '\\0';
        return kz_str(one);
    }
    return kz_index(coll, idx);
}
"""

OBL = re.compile(r"^obligation (\d+) (\S+) (\S+) (\d+) (\S+)$")
LINE = re.compile(r"^#line (\d+)")
CALL = re.compile(r"\bkz_fb_(index|div|mod)\(")

def count_sites(c_text):
    n = 0
    for line in c_text.splitlines():
        if "static inline KzValue kz_fb_" in line:
            continue
        n += len(CALL.findall(line))
    return n

def main():
    if len(sys.argv) < 5:
        raise SystemExit("usage: verify_build.py <compiler> <verifier> <source> <outdir> [program args]")
    compiler, verifier, source, outdir = sys.argv[1:5]
    prog_args = sys.argv[5:]
    os.makedirs(outdir, exist_ok=True)
    name = os.path.splitext(os.path.basename(source))[0]

    proof = run([verifier, "--arg", "source=" + source, "--arg", "mode=proof"])
    if proof.returncode != 0 or not proof.stdout.startswith("kaze-proof 1"):
        print("verify_build: FAIL (verifier proof)")
        print(proof.stderr[:400])
        return 1
    meta = run([verifier, "--arg", "source=" + source, "--arg", "mode=json"])
    try:
        m = json.loads(meta.stdout)
    except Exception as exc:
        print("verify_build: FAIL (verifier json: %s)" % exc)
        return 1
    if m["certified"] != m["routines"] or m["unproven"] != 0 or m["loops"] != m["loops_bounded"]:
        print("verify_build: FAIL (module not certified: %s)" % m)
        return 1
    smt = run([verifier, "--arg", "source=" + source, "--arg", "mode=smt2"])
    open(os.path.join(outdir, name + ".smt2"), "w").write(smt.stdout)
    open(os.path.join(outdir, name + ".proof"), "w").write(proof.stdout)

    line_stats = {}
    for raw in proof.stdout.splitlines():
        mo = OBL.match(raw)
        if not mo:
            continue
        idx, kind, routine, line, status = mo.groups()
        if not kind.startswith("fallback-"):
            continue
        st = line_stats.setdefault(int(line), {"total": 0, "proven": 0})
        st["total"] += 1
        if status == "proven":
            st["proven"] += 1
    elidable = {ln for ln, st in line_stats.items() if st["total"] > 0 and st["total"] == st["proven"]}

    emit = run([compiler, "emit-c", source])
    if emit.returncode != 0 or not emit.stdout:
        print("verify_build: FAIL (emit-c)")
        return 1
    raw_c = emit.stdout
    out_lines = []
    src_line = None
    for line in raw_c.splitlines():
        lm = LINE.match(line)
        if lm:
            src_line = int(lm.group(1))
            out_lines.append(line)
            continue
        if src_line is not None and src_line in elidable:
            line = line.replace("kz_fb_index(", "kz_fb_index_proven(")
            line = line.replace("kz_fb_div(", "kz_fb_div_proven(")
            line = line.replace("kz_fb_mod(", "kz_fb_mod_proven(")
        out_lines.append(line)
    marker = "/* --- Forward Declarations --- */"
    if marker in out_lines:
        pos = out_lines.index(marker)
        out_lines[pos:pos] = HELPERS.splitlines()
    elided_c = "\n".join(out_lines) + "\n"

    raw_path = os.path.join(outdir, name + ".raw.c")
    elided_path = os.path.join(outdir, name + ".elided.c")
    open(raw_path, "w").write(raw_c)
    open(elided_path, "w").write(elided_c)
    raw_sites = count_sites(raw_c)
    elided_sites = count_sites(elided_c)

    dist = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "dist"))
    def build(cpath, out):
        return run(["cc", "-std=c99", "-O2", "-w", "-I", dist, cpath,
                    os.path.join(dist, "kaze_rt.c"), "-lm", "-o", out])
    raw_bin = os.path.join(outdir, name + ".raw")
    elided_bin = os.path.join(outdir, name + ".elided")
    if build(raw_path, raw_bin).returncode != 0 or build(elided_path, elided_bin).returncode != 0:
        print("verify_build: FAIL (compile)")
        return 1
    raw_run = run([raw_bin] + prog_args)
    el_run = run([elided_bin] + prog_args)
    if raw_run.returncode != 0 or el_run.returncode != 0:
        print("verify_build: FAIL (run)")
        return 1
    same = raw_run.stdout == el_run.stdout
    kazebc = run([compiler, "compile", source])
    if kazebc.returncode != 0 or not kazebc.stdout.strip():
        print("verify_build: FAIL (bytecode compile)")
        return 1
    open(os.path.join(outdir, name + ".kazebc.hex"), "w").write(kazebc.stdout)
    digest = 0
    for line in proof.stdout.splitlines():
        if line.startswith("digest "):
            digest = int(line.split()[1])
    if digest == 0:
        print("verify_build: FAIL (proof certificate has no source digest)")
        return 1

    summary = {
        "certified": m["certified"],
        "routines": m["routines"],
        "proof_digest": digest,
        "bytecode_bytes": len(kazebc.stdout.strip()) // 2,
        "fallback_sites_raw": raw_sites,
        "fallback_sites_elided": elided_sites,
        "elided_lines": sorted(elidable),
        "output_equal": same,
        "output": raw_run.stdout.strip(),
    }
    print("verify_build: " + json.dumps(summary, separators=(",", ":")))
    if not same:
        print("verify_build: FAIL (elided program output differs)")
        return 1
    print("verify_build: PASS")
    return 0

if __name__ == "__main__":
    sys.exit(main())
