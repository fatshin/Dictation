# Model comparison

As of 2026-04. Candidate list for Phase 0.

## Selection criteria

- ≤ 4 B parameters, INT4 quantizable
- Official or community-provided ONNX build
- Commercially-usable license (or clearly-scoped license we can accept)
- Japanese + English capability (preferably first-party support)
- Plausible TTFT < 2.5 s on mid-range consumer hardware

## Tier 1 — primary candidates

### Gemma 4 E4B (4.5 B effective)

- **License**: Apache 2.0 on the ONNX repo, but the weights carry the Gemma Prohibited Use Policy — review before shipping.
- **ONNX**: `onnx-community/gemma-4-E4B-it-ONNX` (existence verified via HF API 2026-04-19)
- **Languages**: first-party Japanese support
- **Notable**: Native audio input capability (`audio_encoder_q4.onnx`, `vision_encoder_q4.onnx`, `embed_tokens_q4.onnx`, `decoder_model_merged_q4.onnx`) — opens the door to a one-stage audio→text→rewrite pipeline
- **Concern**: Audio path in ONNX Runtime GenAI may not be wired up in Rust `ort` yet; Phase 0 validates text path first

### Gemma 4 E2B (2 B effective)

- Same family as E4B, lighter. Useful as a low-memory fallback and for baseline-quality comparison
- Good for 8 GB machines
- **ONNX**: `onnx-community/gemma-4-E2B-it-ONNX`

### Gemma 3n E4B (backup)

- Google's prior-generation mobile family (pre-Gemma-4). Kept as an insurance candidate in case Gemma 4 hits a blocker (ONNX Runtime GenAI compat, audio path, license interpretation)
- **Weights**: `google/gemma-3n-E4B-it`
- **License**: Gemma Terms of Use (not Apache 2.0)
- Trigger: promote to Tier 1 only if Gemma 4 E4B cannot be loaded or fails the Phase 0 gate

### Phi-4-mini-instruct (3.8 B)

- **License**: MIT
- **ONNX**: `microsoft/Phi-4-mini-instruct-onnx` (CPU / GPU / mobile variants), plus NPU-specific builds
- **NPU variants available**:
  - `lokinfey/Phi-4-mini-onnx-qnn-npu` — Snapdragon
  - `FluidInference/phi-4-mini-instruct-int4-ov-npu` — Intel OpenVINO
  - `NexaAI/phi4-mini-npu-turbo` — 128K context, Qualcomm
- **Languages**: Japanese explicitly supported
- **Strength**: best Windows NPU story out of the candidates

### Qwen3 4B Instruct 2507 (4 B)

- **License**: verify on model card
- **ONNX**: `onnx-community/Qwen3-4B-Instruct-2507-ONNX` (existence verified via HF API 2026-04-19)
- **Strength**: strong multilingual including Japanese; prior Qwen versions already proven in production with Japanese business text
- **Concern**: ONNX export for newer Qwen variants has had reported issues on some Ryzen AI targets; verify the specific repo's file layout on Day 1

### SmolLM3-3B (fallback, English-only long-form)

- **License**: Apache 2.0
- **ONNX**: `HuggingFaceTB/SmolLM3-3B-ONNX` (Q4 included)
- **Strength**: 128 K context, well-suited for long-form English summarization
- **Concern**: **Officially supports 6 languages only (English, French, Spanish, German, Italian, Portuguese). No Japanese.** Not suitable for this app's primary Japanese use case; kept as a fallback for English-only long summarization workloads

## Tier 2 — fallback

### Llama 3.2 3B

- **License**: Llama 3.2 Community License (text OK in EU; multimodal restricted in EU)
- **ONNX**: Meta-quantized INT4 variants on NGC (NVIDIA) and ai-hub (Qualcomm)
- **Strength**: widely-validated, phone-class ONNX proven
- **Concern**: Japanese quality trails Gemma 4 / Qwen3 in community evals

### Qwen3 4B

- **License**: Apache 2.0
- **ONNX**: `Qwen/Qwen3-*-Instruct-ONNX`, Intel OpenVINO NPU reference
- **Strength**: strong multilingual, first-party Intel NPU coverage
- **Concern**: ONNX export path for newer architectures has had reported issues on Ryzen AI

## Excluded

| Model | Why |
|---|---|
| Exaone 3.5 family | Research-only license |
| Aya Expanse | CC-BY-NC (non-commercial) |
| Command R7B | CC-BY-NC-4.0 |
| Mistral Small 3 24B | Too large for the TTFT budget |
| Llama 4 Scout 17B MoE | ONNX immature, edge-size borderline |
| IBM Granite 3.x | No official ONNX build; self-convert cost too high for Phase 0 |
| Japanese-specialist models (Sarashina, Swallow, ELYZA Llama 3 JP, Stockmark) | No official ONNX. Revisit only if Tier 1 doesn't meet Japanese-quality bar |
| Exaone-Deep | Same research-license issue |

## Model file sizes (approximate INT4 ONNX)

| Model | Size |
|---|---|
| Gemma 4 E2B | ~1.4 GB |
| Gemma 4 E4B | ~2.8 GB |
| Phi-4-mini | ~2.2 GB |
| SmolLM3-3B | ~2.0 GB |
| Llama 3.2 3B | ~2.0 GB |
| Qwen3 4B | ~2.5 GB |

Total for downloading all Tier 1 candidates: ~8.4 GB. The app does **not** bundle any model; each is downloaded on first use with SHA-256 verification and the user can delete any they don't want.

## Two-stage vs one-stage

Two candidate pipelines will be benchmarked in Phase 0:

```
Two-stage:  audio → Whisper (large-v3-turbo) → text → LLM (text-only) → formatted text
One-stage:  audio → Gemma 4 E4B (audio-native)                        → formatted text
```

If Gemma 4 E4B's audio-native input is available in ONNX Runtime GenAI at PoC time, one-stage saves a pipeline hop and may improve TTFT. Two-stage remains the default because Whisper has well-known Japanese accuracy.

## License scanning policy

Every candidate is re-checked against the license file in its HF repo at Phase 0 Day 1. If any license is less permissive than documented here, we document the deviation in `research/phase0/results/licenses.md` and adjust before Phase 1.
