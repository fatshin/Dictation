# OSS landscape — offline dictation + local LLM rewrite

Survey conducted 2026-04-21. Purpose: determine whether the Phase-0/1 build
has a drop-in OSS reference or needs bespoke integration. The survey drove
ADR-001 (runtime pivot to `candle` + `whisper-rs`).

## Direct matches (offline · Whisper · local LLM rewrite · desktop)

| Repo | Stars | License | Stack | Relevance |
|---|---|---|---|---|
| [cjpais/Handy](https://github.com/cjpais/Handy) | 20.3k | MIT | Tauri + React + Rust + `whisper-rs` + `cpal` + `vad-rs` | **ASR pipeline reference**. Forkable by design. Win/macOS/Linux. No LLM-rewrite layer. |
| [alan890104/sumi](https://github.com/alan890104/sumi) | 16 | GPLv3 | Tauri 2 + Svelte + Rust + `candle` + `whisper-rs` + per-app presets | **Closest full-stack match**. Actual Phi-4-mini / Qwen3 / Gemma3 GGUF execution via candle. GPL → *design* reference, not code copy. |
| [moinulmoin/voicetypr](https://github.com/moinulmoin/voicetypr) | 364 | — | Tauri + React + Rust + Whisper | Cross-OS hotkey + cursor injection patterns. LLM rewrite is Groq/Gemini API → not offline. |
| [sypsyp97/light-whisper](https://github.com/sypsyp97/light-whisper) | 13 | **CC BY-NC 4.0** | Tauri 2 + Python sidecar + SenseVoice/faster-whisper | Non-commercial license → excluded. Python sidecar increases complexity. |

## Component references

- **ASR**: `whisper-rs` (whisper.cpp Rust binding). Used by both Handy and
  sumi. Handy's `cpal` + `vad-rs` + VAD-gated transcription pipeline is the
  cleanest reference for real-time dictation.
- **LLM**: `candle-transformers` v0.9.x. `quantized_phi3.rs`,
  `quantized_gemma3.rs`, `quantized_qwen3.rs`, `quantized_llama.rs` all
  present. sumi's `src/models/phi4.rs` patches the fused-QKV tensor layout
  that upstream candle does not yet split for Phi-4.
- **Tokenizer**: HF `tokenizers` crate, interops cleanly with candle.

## What's *not* there (we build)

- **Japanese 敬体 rewrite prompt library with judge**. No OSS project pairs
  JP-dictation-style raw input with a graded rewrite dataset + LLM-as-judge
  harness. The Phase-0 corpus under `research/phase0/inputs/` is ours.
- **4-axis judge with prompt versioning + SQLite cache**. `quality_judge.py`
  is bespoke.
- **Append-only bench schema with EP-provenance + session IDs**. Day-2.5
  rework; no reference.
- **Hard-line gate (`aggregate.py`) enforcing TTFT p95 + quality floor +
  RAM ceiling simultaneously**. Bespoke.
- **`ort` v2 Rust spike** (`research/phase0/rust_ort_spike/`). Kept for
  audit; superseded by ADR-001's candle pivot.

## Ecosystem gaps (risks for the project)

1. **No official Rust binding for `onnxruntime-genai`** as of 2026-04.
   Rust users targeting ONNX GenAI models must hand-write prefill + KV-cache
   against raw `ort`. Motivates ADR-001.
2. **No candle backend for Windows ARM / QNN / DirectML**. Snapdragon X
   laptops fall back to CPU under candle. Surfaced explicitly in ADR-001.
3. **No community Gemma-4 / Qwen-3 `genai_config.json` port** — these
   models' onnx-community exports are Optimum-style, unusable from
   `onnxruntime-genai`. The candle GGUF path sidesteps this entirely.
4. **candle API stability**: pin to `=0.9.2` (matches sumi's lock).

## License notes

- **Handy (MIT)**: we may copy ASR pipeline code directly.
- **sumi (GPLv3)**: design-pattern reference only. Do not copy `polisher.rs`,
  `phi4.rs`, or similar files verbatim into our Apache/MIT-intended codebase
  without re-implementing. Cite the pattern, rewrite the code.
- Candle and whisper-rs are both MIT / Apache-2 compatible.

## Recommended direction (accepted by ADR-001)

- **ASR**: fork (or re-implement, as permitted) Handy's `whisper-rs` +
  `cpal` + `vad-rs` pipeline.
- **LLM**: candle v0.9.2 pinned, GGUF models, Metal on macOS / CUDA on
  Windows x86 / CPU fallback on Windows ARM.
- **UX shell**: Tauri 2 (already in the plan; matches all three references).
- **Retained bespoke layers**: corpus, prompts, judge, bench DB schema,
  aggregate/gate logic.
