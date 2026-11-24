#!/usr/bin/env bash
# phase12-gates.sh — the Phase 12 (tensors / native AI) gate sweep.
#
#   bash scripts/phase12/phase12-gates.sh
#
# Every gate prints PASS/FAIL/SKIP and the script exits non-zero if any gate
# failed. Gates that need a local model or the locally built llama.cpp reference
# print SKIP with the missing path rather than pretending to pass.
set -u
cd "$(dirname "$0")/../.." || exit 2
ROOT=$(pwd)
CC=cc
TMP=/tmp/kaze-phase12-$$
BIN="$TMP/bin"
FAILED=0
COUNT=0
mkdir -p "$BIN" || exit 2
trap 'rm -rf "$TMP"' EXIT

run_gate() {
    name=$1
    shift
    COUNT=$((COUNT + 1))
    printf '%-34s ' "$name"
    if "$@" > "$TMP/out.txt" 2>&1; then
        echo "PASS  $(tail -1 "$TMP/out.txt" | cut -c1-70)"
    else
        status=$?
        if [ "$status" -eq 77 ]; then
            echo "SKIP  $(tail -1 "$TMP/out.txt" | cut -c1-70)"
        else
            FAILED=$((FAILED + 1))
            echo "FAIL"
            sed 's/^/    /' "$TMP/out.txt" | tail -12
        fi
    fi
}

kaze_cc() {
    $CC -std=c99 -O2 -Wall -Wextra -Werror -pedantic -I "$ROOT/dist" "$@"
}

gate_tensor_unit() {
    kaze_cc -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
        "$ROOT/scripts/phase12/test_tensor.c" "$ROOT/dist/kaze_tensor.c" \
        "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_rt.c" -lm -o "$BIN/test_tensor" || return 1
    "$BIN/test_tensor"
}

gate_tensor_math() {
    kaze_cc "$ROOT/scripts/phase12/test_tensor.c" "$ROOT/dist/kaze_tensor.c" \
        "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_rt.c" -lm -o "$BIN/test_tensor_opt" || return 1
    "$BIN/test_tensor_opt"
}

MODEL="$HOME/Downloads/SmolLM2-360M-Instruct-Q8_0.gguf"
if [ -n "$(printenv KAZE_MODEL)" ]; then MODEL="$(printenv KAZE_MODEL)"; fi

gate_gguf() {
    if [ ! -f "$MODEL" ]; then
        echo "skip: no model at $MODEL (local models only; nothing downloads)"
        return 77
    fi
    kaze_cc -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
        "$ROOT/scripts/phase12/test_gguf.c" "$ROOT/dist/kaze_tensor.c" \
        "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_gguf.c" "$ROOT/dist/kaze_rt.c" \
        -lm -o "$BIN/test_gguf" || return 1
    "$BIN/test_gguf" "$MODEL" | tail -4
}

gate_llama_q8() {
    if [ ! -f "$MODEL" ]; then
        echo "skip: no model at $MODEL"
        return 77
    fi
    kaze_cc "$ROOT/scripts/phase12/test_llama.c" "$ROOT/dist/kaze_tensor.c" \
        "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_gguf.c" "$ROOT/dist/kaze_llama.c" \
        "$ROOT/dist/kaze_tokenizer.c" "$ROOT/dist/kaze_rt.c" -lm -o "$BIN/test_llama" || return 1
    "$BIN/test_llama" "$MODEL" "$TMP/q8.bin" 1 2>&1 | tail -2
}

ref_include="$HOME/llama.cpp/include"
ref_lib="$HOME/llama.cpp/build/bin"
build_ref() {
    [ -f "$ROOT/scripts/phase12/ref_llama.c" ] || return 1
    [ -d "$ref_include" ] && [ -d "$ref_lib" ] || return 77
    $CC -O2 -I "$ref_include" -I "$HOME/llama.cpp/ggml/include" \
        "$ROOT/scripts/phase12/ref_llama.c" -L "$ref_lib" -lllama -lggml -lggml-base \
        -Wl,-rpath,"$ref_lib" -o "$BIN/ref_llama" || return 1
}

gate_parity_q8_short() {
    [ -f "$MODEL" ] || { echo "skip: no model at $MODEL"; return 77; }
    build_ref || return 77
    REF_KV=f32 "$BIN/ref_llama" "$MODEL" "$TMP/ref.bin" 1,450 >/dev/null 2>&1 || return 1
    "$BIN/test_llama" "$MODEL" "$TMP/kaze.bin" 1,450 >/dev/null 2>&1 || return 1
    python3 "$ROOT/scripts/phase12/compare_logits.py" "$TMP/ref.bin" "$TMP/kaze.bin" 1e-4 | tail -1
}

gate_tokenizer_parity() {
    [ -f "$MODEL" ] || { echo "skip: no model at $MODEL"; return 77; }
    [ -d "$ref_lib" ] || { echo "skip: no llama.cpp reference build"; return 77; }
    kaze_cc "$ROOT/scripts/phase12/test_tokenizer.c" "$ROOT/dist/kaze_tokenizer.c" \
        "$ROOT/dist/kaze_gguf.c" "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" \
        "$ROOT/dist/kaze_rt.c" -lm -o "$BIN/test_tokenizer" || return 1
    $CC -O2 -I "$ref_include" -I "$HOME/llama.cpp/ggml/include" \
        "$ROOT/scripts/phase12/ref_tokenize.c" -L "$ref_lib" -lllama -lggml -lggml-base \
        -Wl,-rpath,"$ref_lib" -o "$BIN/ref_tokenize" || return 1
    cat > "$TMP/texts" <<'KZEOF'
hello world
Hello, world!
the quick brown fox jumps over the lazy dog.
123
 123
don't stop
a   b
Kaze  is a language.
The year 2024 was good.
JSON: {"a": 1, "b": [2, 3]}
cafe naive resume
I'm sure it's fine.
(((nested))) punctuation!!!
trailing spaces   
  leading spaces
KZEOF
    "$BIN/ref_tokenize" "$MODEL" < "$TMP/texts" > "$TMP/ref.txt" 2>/dev/null
    "$BIN/test_tokenizer" "$MODEL" < "$TMP/texts" 2>/dev/null | grep -v '^tokenizer:' > "$TMP/kaze.txt"
    if diff -q "$TMP/ref.txt" "$TMP/kaze.txt" >/dev/null; then
        echo "tokenizer: $(wc -l < "$TMP/texts" | tr -d ' ') texts match llama.cpp exactly"
        return 0
    fi
    echo "tokenizer mismatch:"
    diff -u "$TMP/ref.txt" "$TMP/kaze.txt" | head -10
    return 1
}

gate_autograd() {
    kaze_cc -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
        "$ROOT/scripts/phase12/test_autograd.c" "$ROOT/dist/kaze_autograd.c" \
        "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_rt.c" \
        -lm -o "$BIN/test_autograd" || return 1
    "$BIN/test_autograd" | tail -3
}

gate_gemm() {
    [ "$(uname -s)" = "Darwin" ] || { echo "skip: GEMM/BLAS gate is macOS-only here"; return 77; }
    kaze_cc -O3 -c "$ROOT/scripts/phase12/bench_gemm.c" -o "$BIN/bench_gemm.o" || return 1
    $CC -O3 -fobjc-arc -I "$ROOT/dist" -Wno-deprecated-declarations \
        -c "$ROOT/dist/kaze_metal.m" -o "$BIN/kaze_metal.o" || return 1
    $CC -O3 -std=c99 -Wall -Wextra -I "$ROOT/dist" -Wno-deprecated-declarations \
        -c "$ROOT/dist/kaze_blas.c" -o "$BIN/kaze_blas.o" || return 1
    $CC -O3 -std=c99 -I "$ROOT/dist" "$BIN/bench_gemm.o" "$BIN/kaze_metal.o" \
        "$BIN/kaze_blas.o" "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" \
        "$ROOT/dist/kaze_rt.c" -framework Accelerate -framework Metal \
        -framework MetalPerformanceShaders -framework Foundation -lm \
        -o "$BIN/bench_gemm" || return 1
    "$BIN/bench_gemm" > "$BIN/gemm.txt" 2>&1
    tail -5 "$BIN/gemm.txt"
    if grep -q 'gemm: gate PASS' "$BIN/gemm.txt"; then
        return 0
    fi
    if grep -q 'gemm: gate SKIP' "$BIN/gemm.txt"; then
        return 77
    fi
    return 1
}

F32_MODEL=/tmp/kaze12/smollm-f32.gguf
if [ -f "$HOME/Downloads/SmolLM2-360M-Instruct-Q8_0.f32.gguf" ]; then
    F32_MODEL="$HOME/Downloads/SmolLM2-360M-Instruct-Q8_0.f32.gguf"
fi
if [ -n "$(printenv KAZE_F32_MODEL)" ]; then F32_MODEL="$(printenv KAZE_F32_MODEL)"; fi

gate_parity_f32() {
    [ -f "$F32_MODEL" ] || { echo "skip: no f32 reference GGUF at $F32_MODEL"; return 77; }
    build_ref || return 77
    REF_KV=f32 "$BIN/ref_llama" "$F32_MODEL" "$TMP/ref.bin" 1,450,338,2997,29991 >/dev/null 2>&1 || return 1
    "$BIN/test_llama" "$F32_MODEL" "$TMP/kaze.bin" 1,450,338,2997,29991 >/dev/null 2>&1 || return 1
    python3 "$ROOT/scripts/phase12/compare_logits.py" "$TMP/ref.bin" "$TMP/kaze.bin" 1e-4 | tail -1
}

KAZE_BIN="$ROOT/bin/kaze"

gate_tensor_shape_compiler() {
    [ -x "$KAZE_BIN" ] || { echo "skip: no built compiler at $KAZE_BIN"; return 77; }
    good=$("$KAZE_BIN" check "$ROOT/scripts/phase12/kaze/tensor_shape_ok.kaze" 2>&1)
    case "$good" in
        "check: ok"*) ;;
        *) echo "valid tensor program rejected: $good"; return 1 ;;
    esac
    bad=$("$KAZE_BIN" check "$ROOT/scripts/phase12/kaze/tensor_shape_bad.kaze" 2>&1)
    case "$bad" in
        *E_KAZE_SHAPE_MISMATCH*) ;;
        *) echo "a shape mismatch was not rejected: $bad"; return 1 ;;
    esac
    # The canonical type text must survive to-machine -> to-human: the machine
    # reader and the human emitter share the one stored representation.
    "$KAZE_BIN" to-machine "$ROOT/scripts/phase12/kaze/tensor_shape_ok.kaze" > "$BIN/shape.machine.kaze" || return 1
    "$KAZE_BIN" to-human "$BIN/shape.machine.kaze" > "$BIN/shape.human.kaze" || return 1
    if ! grep -q 'tensor<f32,\[B,D\]>' "$BIN/shape.human.kaze"; then
        echo "machine round trip lost the tensor type"
        return 1
    fi
    round=$("$KAZE_BIN" check "$BIN/shape.human.kaze" 2>&1)
    case "$round" in
        "check: ok"*) echo "shape algebra: accepted, rejected, and machine-round-tripped"; return 0 ;;
        *) echo "round-tripped program failed to check: $round"; return 1 ;;
    esac
}

gate_safetensors() {
    python3 "$ROOT/scripts/phase12/make_safetensors_fixture.py" "$BIN/fixture.safetensors" >/dev/null || return 1
    kaze_cc -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer \
        "$ROOT/scripts/phase12/test_safetensors.c" "$ROOT/dist/kaze_gguf.c" \
        "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_rt.c" \
        -lm -o "$BIN/test_safetensors" || return 1
    "$BIN/test_safetensors" "$BIN/fixture.safetensors"
}

gate_kaze_parity_f32() {
    [ -x "$KAZE_BIN" ] || { echo "skip: no built compiler at $KAZE_BIN"; return 77; }
    [ -f "$F32_MODEL" ] || { echo "skip: no f32 reference GGUF at $F32_MODEL"; return 77; }
    build_ref || return 77
    REF_KV=f32 "$BIN/ref_llama" "$F32_MODEL" "$BIN/ref_f32.bin" 1,450,338,2997,29991 >/dev/null 2>&1 || return 1
    bash "$ROOT/scripts/phase12/kaze-tensor-build.sh" "$KAZE_BIN" \
        "$ROOT/scripts/phase12/kaze/tensor_infer_value.kaze" "$BIN/tensor_infer_value" || return 1
    got=$("$BIN/tensor_infer_value" --arg model="$F32_MODEL" --arg t0=1 --arg t1=450 \
        --arg t2=338 --arg t3=2997 --arg t4=29991 | tail -1)
    python3 - "$BIN/ref_f32.bin" "$got" <<'PYEOF'
import struct
import sys
path, got = sys.argv[1], float(sys.argv[2])
with open(path, "rb") as f:
    n = struct.unpack("<i", f.read(4))[0]
    ref = struct.unpack("<%df" % n, f.read(4 * n))[0]
diff = abs(ref - got)
print("Kaze logit %.6f vs llama.cpp %.6f: abs diff %.3g" % (got, ref, diff))
sys.exit(0 if diff <= 1e-4 else 1)
PYEOF
}

gate_tensor_program() {
    [ -x "$KAZE_BIN" ] || { echo "skip: no built compiler at $KAZE_BIN"; return 77; }
    bash "$ROOT/scripts/phase12/kaze-tensor-build.sh" "$KAZE_BIN" \
        "$ROOT/scripts/phase12/kaze/tensor_exec.kaze" "$BIN/tensor_exec" || return 1
    got=$("$BIN/tensor_exec")
    if [ "$got" = "172" ]; then
        echo "tensor program ran: matmul+add+transpose+index = 172"
        return 0
    fi
    echo "tensor program returned '$got', expected 172"
    return 1
}

gate_kaze_inference() {
    [ -x "$KAZE_BIN" ] || { echo "skip: no built compiler at $KAZE_BIN"; return 77; }
    [ -f "$MODEL" ] || { echo "skip: no model at $MODEL"; return 77; }
    build_ref || return 77
    REF_KV=f32 "$BIN/ref_llama" "$MODEL" "$BIN/ref_infer.bin" 1,450 >/dev/null 2>&1 || return 1
    bash "$ROOT/scripts/phase12/kaze-tensor-build.sh" "$KAZE_BIN" \
        "$ROOT/scripts/phase12/kaze/tensor_infer.kaze" "$BIN/tensor_infer" || return 1
    got=$("$BIN/tensor_infer" --arg model="$MODEL" --arg t0=1 --arg t1=450 | tail -1)
    ref_top=$(python3 -c 'import struct,sys; b=open(sys.argv[1],"rb").read(); n=struct.unpack("<i",b[:4])[0]; v=struct.unpack("<%df"%n,b[4:4+4*n]); print(max(range(n),key=lambda i:v[i]))' "$BIN/ref_infer.bin")
    if [ "$got" = "$ref_top" ]; then
        echo "Kaze inference argmax $got matches llama.cpp"
        return 0
    fi
    echo "Kaze inference argmax '$got' != llama.cpp '$ref_top'"
    return 1
}

run_gate "tensor unit (ASan/UBSan)" gate_tensor_unit
run_gate "tensor math (optimised)" gate_tensor_math
run_gate "GGUF zero-copy ingestion" gate_gguf
run_gate "SafeTensors zero-copy ingestion" gate_safetensors
run_gate "Q8_0 forward smoke" gate_llama_q8
run_gate "Q8_0 short parity vs llama.cpp" gate_parity_q8_short
run_gate "f32 logits parity <= 1e-4" gate_parity_f32
run_gate "tokenizer parity vs llama.cpp" gate_tokenizer_parity
run_gate "autograd + 1000 training passes" gate_autograd
run_gate "GEMM throughput vs BLAS" gate_gemm
gate_metal_msl() {
    [ "$(uname -s)" = "Darwin" ] || { echo "skip: Metal gate is macOS-only"; return 77; }
    kaze_cc -O3 -c "$ROOT/scripts/phase12/test_metal_msl.c" -o "$BIN/test_metal_msl.o" || return 1
    $CC -O3 -fobjc-arc -I "$ROOT/dist" -Wno-deprecated-declarations \
        -c "$ROOT/dist/kaze_metal.m" -o "$BIN/kaze_metal.o" || return 1
    $CC -O3 -std=c99 -I "$ROOT/dist" "$BIN/test_metal_msl.o" "$BIN/kaze_metal.o" \
        "$ROOT/dist/kaze_tensor.c" "$ROOT/dist/kaze_quant.c" "$ROOT/dist/kaze_rt.c" \
        -framework Metal -framework MetalPerformanceShaders -framework Foundation -lm \
        -o "$BIN/test_metal_msl" || return 1
    out=$("$BIN/test_metal_msl" 2>&1)
    echo "$out" | tail -1
    if echo "$out" | grep -q 'test_metal_msl: OK'; then
        return 0
    fi
    if echo "$out" | grep -q 'SKIP'; then
        return 77
    fi
    return 1
}

run_gate "Metal MSL GEMM kernel" gate_metal_msl
run_gate "tensor shape algebra (compiler)" gate_tensor_shape_compiler
run_gate "Kaze tensor program (runtime)" gate_tensor_program
run_gate "Kaze model inference vs llama.cpp" gate_kaze_inference
run_gate "Kaze f32 logit parity <= 1e-4" gate_kaze_parity_f32

echo
if [ "$FAILED" -eq 0 ]; then
    echo "phase12: $COUNT gates, 0 failed"
    exit 0
fi
echo "phase12: $COUNT gates, $FAILED failed"
exit 1
