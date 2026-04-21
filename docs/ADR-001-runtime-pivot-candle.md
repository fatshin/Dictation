# ADR-001 — LLM runtime: pivot from `ort` (ONNX Runtime GenAI) to `candle`

- **Status**: Accepted (2026-04-21)
- **Scope**: Phase 1 LLM inference path. Affects ROADMAP Phase 1a/1b effort
  estimates and Windows hardware-acceleration story.
- **Supersedes**: The Phase-0 plan's "ort v2 + manual KV-cache loop" in
  `PHASE0_POC.md §Phase 1 runtime risk`.

## Context

Phase 0 Day 3 left two defects in the original runtime path:

1. `onnxruntime-genai 0.13.1` on macOS hit a CoreML-specific file-path bug
   (`model.onnx/model.onnx.data: Not a directory`) that we could not resolve
   from the Python side. All 276 measured runs fell back to CPU EP.
2. `onnxruntime-genai` ships no official Rust binding. Phase 1 was to
   re-implement prefill + KV-cache decode in raw `ort` v2 — a 1-2 week spike
   with real tokenizer/template alignment risk, and a distinct CoreML bug
   surface of its own.

A 20-minute OSS survey (documented in `OSS_LANDSCAPE.md`) surfaced
`alan890104/sumi`: a Tauri 2 + Rust app doing the same two-stage ASR → LLM
rewrite flow using **`candle`** (LLM) + **`whisper-rs`** (ASR). candle ships
first-class Phi-4-mini / Gemma 3-4 / Qwen 3 / Llama 3 support, built-in
Metal backend, and GGUF loader. It does not need a `genai_config.json` —
the per-model `quantized_*.rs` handles layer wiring directly.

## Decision

Replace `ort` (ONNX Runtime GenAI) with `candle` for the LLM layer. Keep
ASR unified on `whisper-rs` (= whisper.cpp binding) across macOS and
Windows, retiring the two-sidecar (WhisperKit + sherpa-onnx) plan.

- **LLM runtime**: `candle-core 0.9.x` + `candle-transformers 0.9.x`
  (version-pinned; API is not yet stable across minors).
- **Model format**: GGUF. Phi-4-mini uses the `bartowski` / community GGUF
  conversions of Microsoft's weights; Gemma-3, Qwen-3, Llama-3.2 likewise
  have GGUF from HF community.
- **Hardware acceleration**:
  - macOS: Metal (candle `metal` feature).
  - Windows x86 w/ NVIDIA: CUDA (candle `cuda` feature).
  - **Windows ARM (Snapdragon X / Copilot+ PCs): out of scope** — the
    2026-04-21 scope decision is to target Windows x86 (CUDA/CPU) and
    macOS (Metal). Snapdragon X is not supported in Phase 1 and is not a
    Phase-1-gate concern. A future phase can revisit via a dual-runtime
    ONNX-GenAI QNN path if user demand materializes.
- **ASR runtime**: `whisper-rs` on both OSes; drop WhisperKit CLI sidecar
  and `sherpa-onnx` from active Phase 1 scope.

## Consequences

### Benefits

- Eliminates the "port onnxruntime-genai's prefill + decode into raw ort"
  engineering spike (estimated 1-2 weeks, real token-template risk).
- Eliminates the Python-vs-Rust runtime divergence: Phase-0 Python bench
  previously did not predict Phase-1 Rust performance. With candle we bench
  and ship the same runtime.
- Unifies ASR across OSes → Phase 1b (Windows parity) shrinks from a full
  sherpa-onnx integration to dependency + code-signing work. Estimated
  saving: ~1 week on Phase 1b.
- candle's `quantized_*.rs` removes the `genai_config.json` dependency that
  blocked Gemma-4-E2B / Qwen3-4B (onnx-community repos don't ship it).
  Those models re-enter the model-selection pool, loaded from GGUF.

### Costs

- **Windows ARM hardware acceleration is lost.** candle 0.9 has no QNN,
  DirectML, or Vulkan backend. On Snapdragon X the only path is CPU, and
  a 3-4 B INT4 model on ARM-CPU is likely to miss the `TTFT < 2500 ms`
  hard line. This has to be explicitly acknowledged before Phase 1b start.
  Options at that point: (a) accept degraded Windows ARM experience,
  (b) keep an `onnxruntime-genai` path *just* for Windows ARM, dual-runtime,
  (c) defer Windows ARM to a later phase.
- All Phase-0 ONNX benchmark numbers become *reference only* — the Phase-1
  runtime is different. We keep the corpus, the prompt templates, the
  judge, and the harness structure; the inference layer under
  `bench_llm.py` has to be rewritten against candle before any Day-4/5
  bench re-run is valid.
- candle API is not stable — we pin to `=0.9.2` (matching sumi's lock) and
  track breaking changes manually.
- Phi-4 GGUF has fused QKV / gate+up tensors. candle's default
  `quantized_phi3.rs` does not split them; we need sumi's patched loader
  (or equivalent) in `src/models/phi4.rs`. Additional per-model glue.
- candle's own `whisper.rs` is slower than whisper.cpp; we confirm (from
  sumi's choice) that the pragmatic path is `whisper-rs` for ASR, not
  candle-whisper. ASR is therefore on a *second* C++ dependency.

### Phase-0 knock-on work

Recorded in `PHASE0_POC.md §Day 4` (to be added):

1. Replace `research/phase0/rust_ort_spike/` with a `candle_spike/` mirror:
   Phi-4-mini GGUF → 1 forward pass → print logits shape + TTFT.
2. Rewrite the Python bench harness's inference core to call out to a small
   Rust binary (or a `pyo3`-wrapped candle, if we're feeling brave) — or
   accept the Python bench as ONNX reference and re-run the 3-model matrix
   in Rust/candle for the actual gate numbers.
3. Reopen `QUARANTINED_TIER_1` in `models.py`: Gemma-4-E2B and Qwen3-4B
   re-enter candidate set via GGUF.
4. Update the model-manifest: we track `.gguf` files, not onnx + onnx.data.

## Rejected alternatives

- **B — ASR unified (whisper-rs) but keep ort for LLM.** Cheaper pivot but
  leaves the onnxruntime-genai Rust-binding problem unsolved and doesn't
  address the CoreML file-path bug. We'd still rewrite the LLM layer from
  Python to Rust but with less runtime ergonomics (no built-in KV-cache).
- **A — Stay the course (WhisperKit + sherpa-onnx + ort v2).** Highest
  implementation cost, highest engineering risk, no precedent in the OSS
  landscape for the JP-dictation-rewrite use case. Rejected.

## References

- Phase-0 Day-2.5/3 Gate draft: `research/phase0/results/gate_decision_draft.md`
- OSS landscape survey: `docs/OSS_LANDSCAPE.md`
- sumi (GPLv3, reference implementation): https://github.com/alan890104/sumi
- Handy (MIT, ASR reference): https://github.com/cjpais/Handy
- candle: https://github.com/huggingface/candle
- whisper-rs: https://github.com/tazz4843/whisper-rs
