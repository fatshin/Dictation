# Phase 0 — Technical PoC

One-week technical validation before committing to Phase 1.

## Purpose

Resolve three unknowns:

1. **Runtime viability**: can ONNX Runtime GenAI hit the latency budget on macOS and Windows?
2. **Model selection**: from the Tier 1 shortlist, which is the primary? Which is the fallback?
3. **ASR path**: two-stage (Whisper → LLM) vs one-stage (audio-native LLM, if available on Gemma 4 E4B)

## Go / No-Go criteria

| Metric | Hard line | Target | Notes |
|---|---|---|---|
| TTFT (first-token latency) | < 2500 ms | < 1500 ms | Measured from audio-stop to first visible token |
| LLM-as-judge quality (4-axis avg / 10) | ≥ 7.0 | ≥ 7.5 | keigo / filler / semantic / structure |
| Japanese WER | < 15 % | < 10 % | On the 20-utterance benchmark set |
| Peak RAM | < 8 GB | < 6 GB | Steady-state while dictating |

No-Go if any line slips below the hard line or fewer than two Tier 1 models clear all lines.

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

| Tier | Model | HF repo (candidate) | INT4 size | License |
|---|---|---|---|---|
| 1 | Gemma 4 E4B | `onnx-community/gemma-4-e4b-it-ONNX` | ~2.8 GB | Gemma TOU |
| 1 | Gemma 4 E2B | `onnx-community/gemma-4-e2b-it-ONNX` | ~1.4 GB | Gemma TOU |
| 1 | Phi-4-mini-instruct | `microsoft/Phi-4-mini-instruct-onnx` | ~2.2 GB | MIT |
| 1 | SmolLM3-3B | `HuggingFaceTB/SmolLM3-3B-Instruct-ONNX` | ~2.0 GB | Apache 2.0 |
| 2 | Llama 3.2 3B | `onnx-community/Llama-3.2-3B-Instruct-ONNX` | ~2.0 GB | Llama 3.2 |
| 2 | Qwen3 4B | `Qwen/Qwen3-4B-Instruct-ONNX` | ~2.5 GB | Apache 2.0 |

Exact repo names and ONNX availability are verified on Day 1 AM before the rest of the plan runs.

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

| Day | Work | Gate |
|---|---|---|
| 1 AM | Verify HF ONNX repos exist for every Tier 1 candidate | If Gemma 4 ONNX is missing, promote Tier 2 immediately |
| 1 PM | Download all models, SHA-256 verify | |
| 2 | Implement `bench_llm.py`, run Tier 1 × Japanese keigo workload | Early TTFT signal |
| 3 | Run Tier 1 × remaining 3 workloads | `bench_db.sqlite` complete |
| 4 AM | Implement `bench_asr.py`, run on macOS | |
| 4 PM | Run ASR bench on Windows (fallback: whisper.cpp if no native sidecar yet) | WER fixed |
| 5 | Judge all outputs, aggregate, write `results/report.md` v1 | Draft verdict |
| 6 | Cross-review (multi-model sanity check) | Critical issues resolved |
| 7 | Phase 1 Go/No-Go report | Decision |

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

- Quality shortfall → improve prompt, try few-shot, retry. If still failing, add Tier 2 (Qwen3 4B).
- TTFT shortfall → go one size down (E4B → E2B, 3B → 1.5B). INT4 → INT8 is a trap; don't.
- Runtime shortfall → switch to `llama.cpp` + GGUF as an emergency Phase 0.5.
