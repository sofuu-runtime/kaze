#!/usr/bin/env python3
"""machine_constrained_gen.py -- plan 16 constrained-generation smoke test.

Runs the local llama.cpp CLI with dist/grammars/kaze_machine.gbnf and checks
that every sample is accepted by the Kaze machine reader.  The prompt is an
incomplete routine, so a successful sample also exercises the expression and
call grammar rather than producing an empty `(kaze 2)`.

No weights are downloaded: the caller points at a GGUF already on this Mac.

    machine_constrained_gen.py --bin bin/kaze --model ~/Downloads/foo.gguf \
        --llama-cli ~/llama.cpp/build/bin/llama-cli \
        --grammar dist/grammars/kaze_machine.gbnf --samples 3
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile

PROMPT = """(kaze 2
(action add ((a num) (b num)) -> num
(return """


def extract(text):
    """Return the last `(kaze 2 ...)` form, closing an EOS-truncated root."""
    starts = [m.start() for m in re.finditer(r"\(kaze 2", text)]
    if not starts:
        return None
    out = text[starts[-1]:]
    # llama-cli prints a timing footer after the generation.
    out = re.split(r"\n?\[ Prompt:|\nExiting\.\.\.", out, maxsplit=1)[0]
    depth = 0
    in_string = False
    escaped = False
    end = None
    i = 0
    while i < len(out):
        c = out[i]
        if in_string:
            if escaped:
                escaped = False
            elif c == "\\":
                escaped = True
            elif c == '"':
                in_string = False
        elif c == '"':
            in_string = True
        elif c in ";#":
            nl = out.find("\n", i)
            i = len(out) if nl < 0 else nl
            continue
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
        i += 1
    if end is not None:
        out = out[:end]
    else:
        out = out.rstrip() + ")" * max(depth, 0)
    return out if "(action" in out or "(record" in out else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--llama-cli", required=True)
    ap.add_argument("--grammar", required=True)
    ap.add_argument("--samples", type=int, default=3)
    ap.add_argument("--tokens", type=int, default=64)
    args = ap.parse_args()

    if not os.path.isfile(args.model):
        print("constrained-gen: missing model %s" % args.model)
        return 2
    ok = 0
    with tempfile.TemporaryDirectory(prefix="kaze-gen.") as tmp:
        for i in range(args.samples):
            run = subprocess.run(
                [args.llama_cli, "-m", args.model, "-p", PROMPT,
                 "--grammar-file", args.grammar, "-n", str(args.tokens),
                 "--temp", "0", "--no-display-prompt", "--no-warmup",
                 "--log-disable", "--single-turn", "--simple-io"],
                input="", text=True, capture_output=True)
            candidate = extract(run.stdout)
            if candidate is None:
                print("constrained-gen: sample %d produced no machine form" % i)
                continue
            path = os.path.join(tmp, "sample%d.kaze" % i)
            with open(path, "w") as fh:
                fh.write(candidate + "\n")
            parse = subprocess.run([args.bin, "to-human", path],
                                   text=True, capture_output=True)
            if parse.returncode != 0:
                print("constrained-gen: sample %d did not parse: %s" %
                      (i, parse.stderr.strip()[:160]))
                continue
            ok += 1
        print("constrained-gen: %d/%d parsed" % (ok, args.samples))
    return 0 if ok == args.samples else 1


if __name__ == "__main__":
    sys.exit(main())
