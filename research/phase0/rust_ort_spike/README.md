# ort-spike

Rust spike for Dictation Phase 0. Validates that the `ort` crate v2 (v2.0.0-rc.10) can load candidate ONNX decoder models and execute at least one forward pass. **Not production code — intentionally minimal.**

## Goals

1. Confirm `ort` v2 loads each Phase 0 candidate (Phi-4-mini, Gemma 4 E4B, Qwen3 4B) without panic.
2. Measure **first-forward latency** (a lower bound on TTFT, not a full generation).
3. Surface the declared input/output names so Phase 1 knows exactly what `past_key_values.*` tensors to wire.
4. Compare numbers against Python `onnxruntime_genai` figures from `bench_llm.py`.

## Non-goals

- Full KV-cache decode loop (Phase 1 work).
- Multi-token generation or sampling (Phase 1 work).
- Execution-provider selection — this spike uses the default CPU EP. CoreML / DirectML / CUDA wiring is marked with `TODO(Phase 1)` in `src/main.rs`.
- Production-grade error recovery, logging, config, benchmarking harness.

## Prerequisites

- Rust stable (>= 1.78). Install via <https://rustup.rs> if missing.
- ONNX Runtime shared library available at runtime (the `load-dynamic` feature defers linking).
  - macOS: `brew install onnxruntime`, or point `ORT_DYLIB_PATH` to the `.dylib`.
  - Linux: system package or `ORT_DYLIB_PATH` to `libonnxruntime.so`.
  - Windows: `ORT_DYLIB_PATH` to `onnxruntime.dll`.
- A candidate model exported as ONNX in the layout produced by Optimum / `onnxruntime-genai`, typically under:
  - `~/.cache/huggingface/hub/models--<org>--<name>-onnx/snapshots/<sha>/.../`

## Build

```bash
cd research/phase0/rust_ort_spike
cargo build --release
```

First build compiles ort + tokenizers from source; expect several minutes on an M-series Mac.

## Run

```bash
# Example: Phi-4-mini INT4 RTN block-32 acc-level-4
cargo run --release -- \
  --model-dir "$HOME/.cache/huggingface/hub/models--microsoft--Phi-4-mini-instruct-onnx/snapshots/<sha>/cpu_and_mobile/cpu-int4-rtn-block-32-acc-level-4" \
  --prompt "Hello, my name is" \
  --n-tokens 8 \
  --output ../results/rust_spike_phi4.json
```

If the ONNX runtime library is not on the default search path:

```bash
ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib \
  cargo run --release -- --model-dir <...> --output <...>
```

## Expected output

- stdout: model path, load time (ms), input token count, first-forward latency (ms), declared input/output names.
- JSON at `--output` containing the same data plus `ran_ok` / `run_error` for scripted comparison.
- Exit code `0` on successful forward pass, `2` if the run failed (likely because the model declares additional required inputs such as `past_key_values.*`). The JSON still gets written in the failure case so you can read `declared_input_names` and plan Phase 1 wiring.

## Known limitations

- Models exported with an explicit `past_key_values.*` input list **will error on this single-forward call** (ort requires all declared inputs). The error message plus the JSON's `declared_input_names` tells you exactly which past-KV tensors to provide. Wiring them is Phase 1 work.
- No sampling: even on success, only the raw logits of the prefill are produced.
- `--n-tokens` is accepted but currently ignored (kept for CLI-contract stability with the Phase 1 generator).
