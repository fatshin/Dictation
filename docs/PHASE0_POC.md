# Phase 0 — Technical PoC

One-week technical validation before committing to Phase 1.

## Purpose

Resolve three unknowns:

1. **Runtime viability**: can ONNX Runtime GenAI hit the latency budget on macOS and Windows?
2. **Model selection**: from the Tier 1 shortlist, which is the primary? Which is the fallback?
3. **ASR path**: two-stage (Whisper → LLM) vs one-stage (audio-native LLM, if available on Gemma 4 E4B)

## Go / No-Go criteria

All latency metrics are split so the blame can land on the right subsystem.

| Metric | Hard line | Target | Defined as |
|---|---|---|---|
| ASR final latency | < 800 ms | < 500 ms | audio_stop → asr_final emit |
| Rewrite TTFT | < 1500 ms | < 800 ms | rewrite_start → first LLM token |
| End-to-end TTFT | < 2500 ms | < 1500 ms | audio_stop → first visible token in target app |
| LLM-as-judge quality (4-axis avg / 10) | ≥ 7.0 | ≥ 7.5 | keigo / filler / semantic / structure |
| Japanese CER | < 10 % | < 7 % | character-level, Japanese-only utterances |
| Mixed-term preservation rate | ≥ 95 % | ≥ 98 % | technical terms retained verbatim in JP/EN mixed utterances |
| English WER | < 10 % | < 7 % | English-only utterances |
| Peak RAM (total: Tauri + ASR sidecar + LLM) | < 8 GB | < 6 GB | Steady-state while dictating |

No-Go if any hard line slips or fewer than two Tier 1 models clear all lines.

## Benchmark workloads

All inputs are authored as plausible raw dictation and paired with an expected rewritten output. Each file carries both and the benchmark runner scores the model output against the reference using an LLM-as-judge.

Location: `research/phase0/inputs/`

### 1. Japanese business-register rewriting — `ja_keigo_01-05.txt` (5 samples)

Casual spoken Japanese → polite written form (email / memo register).

### 2. Mixed JP/EN code-switching — `jp_en_mix_01-05.txt` (5 samples)

Engineer/PM speech with English technical terms interleaved. Test whether rewrites keep a consistent register and preserve the technical terms correctly.

### 3. English business — `en_business_01-05.txt` (5 samples)

Slack-casual English → formal email, with filler removal and sentence completion.

### 4. Long-form summarization — `summary_long_01-03.txt` (3 samples, 5–10 K characters each)

1-on-1 transcripts → 3-line summary + action items.

## Metrics

| Metric | Capture point | Unit |
|---|---|---|
| TTFT | start of inference → first emitted token | ms |
| tokens/sec | full stream duration | tok/s |
| peak RSS | process memory | MB |
| model file size | weights + metadata on disk | GB |
| quality score | 4-axis LLM judge | 0–10 per axis |

Quality judging uses a fixed prompt, `temperature=0`, and a cache keyed on `(model, input_hash, output_hash) → scores` so re-runs don't re-bill the judge.

## Candidate models

| Tier | Model | HF repo (verified 2026-04-19) | INT4 size | License |
|---|---|---|---|---|
| 1 | Gemma 4 E4B | `onnx-community/gemma-4-E4B-it-ONNX` | ~2.8 GB | Apache 2.0 (subject to Gemma Prohibited Use Policy) |
| 1 | Gemma 4 E2B | `onnx-community/gemma-4-E2B-it-ONNX` | ~1.4 GB | Apache 2.0 (same policy) |
| 1 | Phi-4-mini-instruct | `microsoft/Phi-4-mini-instruct-onnx` | ~2.2 GB | MIT |
| 1 | Qwen3 4B Instruct 2507 | `onnx-community/Qwen3-4B-Instruct-2507-ONNX` | ~2.5 GB | verify on model card |
| 2 | Llama 3.2 3B | `onnx-community/Llama-3.2-3B-Instruct-ONNX` | ~2.0 GB | Llama 3.2 |
| 2 | SmolLM3-3B | `HuggingFaceTB/SmolLM3-3B-ONNX` | ~2.0 GB | Apache 2.0 (English + 5 EU languages only; **not for Japanese**) |
| Backup | Gemma 3n E4B | `google/gemma-3n-E4B-it` | ~2.8 GB | Gemma Terms | insurance if Gemma 4 path hits any blocker |

Existence verified via HF API on 2026-04-19. Exact revision, file layout, and `genai_config.json` presence are re-verified on Day 1 AM before the rest of the plan runs.

## Implementation

Directory: `research/phase0/`

```
research/phase0/
├── bench_llm.py          # TTFT / tokens-per-second / quality per model × workload
├── bench_asr.py          # WER on 20 utterances (JP/EN/mixed)
├── bench_e2e.py          # two-stage vs one-stage comparison
├── quality_judge.py      # LLM-as-judge with on-disk cache
├── models.py             # download + SHA-256 verify
├── runtime_selector.py   # EP selection (Mac CoreML / Win DML / Win QNN / Win OV)
├── inputs/               # benchmark corpus
├── recordings/           # WAV samples (git-ignored, ≥ 100 MB total)
└── results/
    ├── bench_db.sqlite   # per-run metrics
    └── report.md         # Day 7 output
```

Minimal call shape:

```python
import onnxruntime_genai as og

model = og.Model(model_dir)
tokenizer = og.Tokenizer(model)
params = og.GeneratorParams(model)
params.set_search_options(max_length=2048, temperature=0.3)
params.input_ids = tokenizer.encode(prompt)

generator = og.Generator(model, params)
t0 = time.perf_counter()
first_token_ms = None
while not generator.is_done():
    generator.compute_logits()
    generator.generate_next_token()
    if first_token_ms is None:
        first_token_ms = (time.perf_counter() - t0) * 1000  # TTFT
```

EP selection is platform-conditional. Sample:

```python
def select_execution_provider() -> list[str]:
    import platform, sys
    if sys.platform == "darwin":
        return ["CoreMLExecutionProvider", "CPUExecutionProvider"]
    if sys.platform == "win32":
        if has_qnn():
            return ["QNNExecutionProvider", "CPUExecutionProvider"]
        if has_openvino():
            return ["OpenVINOExecutionProvider", "CPUExecutionProvider"]
        return ["DmlExecutionProvider", "CPUExecutionProvider"]
    return ["CPUExecutionProvider"]
```

## Schedule

Spread to avoid the known trap of "benchmark everything on day 2". Day 2 is deliberately a smoke pass that narrows the field before the expensive runs.

| Day | Work | Gate |
|---|---|---|
| 1 AM | For each Tier 1 candidate: confirm exact HF repo, license file, pinned revision, `genai_config.json` presence, file layout. Table the findings. | If Gemma 4 ONNX is structurally missing, promote Tier 2 immediately |
| 1 PM | Download all Tier 1 models, SHA-256 verify. Run a 32-token smoke decode on CPU EP for each. | All candidates can decode; dead ones dropped |
| 2 | `bench_llm.py` v1. For each Tier 1 model: run smoke set = keigo 1–2 samples + English business 1 sample. Rank by TTFT + quick judge score. | Top 2 identified |
| 3 | Top 2 only × all 4 workloads, full run. Bottom half gets a light pass to confirm the ranking. | `bench_db.sqlite` complete for top 2 |
| 4 AM | `bench_asr.py` on macOS (WhisperKit sidecar). | JP/EN/mixed WER+CER baseline |
| 4 PM | ASR bench on Windows. If no native sidecar yet: whisper.cpp. | Cross-platform WER baseline fixed |
| 5 | Run `quality_judge.py` over all outputs, aggregate, draft `results/report.md` v1. | Draft verdict in writing |
| 6 | Cross-review (Codex + Gemini + this author). Re-measure anything flagged. | CRITICAL count = 0 |
| 7 | Phase 1 Go/No-Go report, decision recorded in repo. | Decision |

## Risk and fallback

| Risk | Likelihood | Fallback |
|---|---|---|
| Gemma 4 E4B ONNX missing | Medium | Promote Llama 3.2 3B into Tier 1 immediately |
| CoreML EP slow on Mac for 3–4B INT4 | Medium | Add an MLX Swift sidecar path on macOS |
| No Snapdragon X / Core Ultra hardware at hand | High | Benchmark CPU + DirectML only, mark NPU row "pending" |
| Gemma 4 audio-native input not in ONNX | High | Drop one-stage A/B, stick with two-stage; revisit later |
| Judge cost creep | Low | Hard cap via cache + per-session budget ceiling |

## Entry conditions for Phase 1

All of:

1. At least two Tier 1 models hit TTFT < 2500 ms and quality ≥ 7.0
2. A working ASR path (JP WER < 15 %) on both macOS and Windows
3. `runtime_selector` works on both OSes at minimum on CPU EP
4. Phase 0 results committed to the repo as `research/phase0/results/report.md`

If conditions fail:

- Quality shortfall → improve prompt, try few-shot, retry. If still failing, promote a Tier 2 model.
- TTFT shortfall → go one size down (E4B → E2B, 3B → 1.5B). INT4 → INT8 is a trap; don't.
- Runtime shortfall → **Phase 0 is No-Go**, not a fallback. Switching to `llama.cpp` + GGUF invalidates the `trait LlmRuntime` / `ort::Session` / ONNX MANIFEST design and is a full architecture redo. Call a re-design meeting before moving on.

## Phase 1 runtime risk (separate gate)

Phase 0 benchmarks the candidate models using **Python** `onnxruntime_genai`. The Phase 1 production runtime uses **Rust** `ort` crate v2 with a manual KV-cache loop (no official Rust binding for `onnxruntime-genai` exists). The two are not the same binary and the Python results do not perfectly predict Rust performance.

To de-risk:

- Day 1 Lane C: prototype a Rust `ort` v2 spike (`research/phase0/rust_ort_spike/`) — load one chosen model, run one forward pass, measure TTFT. Does not need to be production-quality.
- Expect Rust manual KV-cache loop to give 10–30 % lower tokens/sec than Python `onnxruntime_genai` on the same hardware. Phase 0 Go decision should bake in a 0.7× safety margin on the Python numbers.
- If the Rust spike on Day 1 is >50 % slower than Python, extend Phase 0 by 2–3 days to implement the C API FFI fallback, or acknowledge Rust backend risk and set a Phase 1 Week 1 "runtime-prototype gate" as an explicit No-Go retrigger.
