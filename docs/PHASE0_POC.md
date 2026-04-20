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
├── bench_e2e.py          # two-stage vs one-stage comparison (optional; deferred to Phase 1 if Gemma 4 audio-native ONNX is not ready in the timeframe)
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
| **2.5** | **Pre-Day-3 corrections (see below). Fix data-integrity bugs caught in Day-2 cross-review before running expensive Day-3 bench.** | **CRITICAL/HIGH count = 0 before Day 3** |
| 3 | Top 2 only × all 4 workloads, full run. Bottom half gets a light pass to confirm the ranking. | `bench_db.sqlite` complete for top 2 |
| 4 AM | `bench_asr.py` on macOS (WhisperKit sidecar). | JP/EN/mixed WER+CER baseline |
| 4 PM | ASR bench on Windows. If no native sidecar yet: whisper.cpp. | Cross-platform WER baseline fixed |
| 5 | Run `quality_judge.py` over all outputs, aggregate, draft `results/report.md` v1. | Draft verdict in writing |
| 6 | Cross-review (Codex + Gemini + this author). Re-measure anything flagged. | CRITICAL count = 0 |
| 7 | Phase 1 Go/No-Go report, decision recorded in repo. | Decision |

### Day 2.5 — corrections (added 2026-04-20)

Day-2 cross-review (advisor + Codex + Gemini) flagged issues that would poison
Day 3's larger dataset if left in place. Fix before any further bench runs.

**CRITICAL (must fix before Day 3)**

1. **Prompt template is language-blind.** `DEFAULT_PROMPT_TEMPLATE` does not
   instruct the model to preserve the input language or use Japanese polite
   form. Phi-4-mini emitted English for Japanese inputs in Day 2. Judging
   those outputs would score prompt failure, not model quality. → Add explicit
   "keep input language; use 敬体 for Japanese" directive, per-workload.
2. **`model_id` mislabeled.** `bench_llm.py:288` uses `p.name` when a bare
   path is passed, so all 10 Day-2 rows are stamped
   `cpu-int4-rtn-block-32-acc-level-4` instead of `phi-4-mini`. → Split into
   `model_alias` / `model_variant` / `model_repo` / `model_revision` columns;
   require alias lookup, fall back to `--label` only with explicit opt-in.
3. **`INSERT OR REPLACE` is not audit-safe.** Silently overwrites prior
   `(model_id, workload_id, run_seq)` rows. → Switch to append-only with
   `bench_session_id` (UUID per run) + drop `UNIQUE` constraint.
4. **Existing 10 rows are gate-ineligible.** Quarantine as
   `results/bench_db.invalid_YYYYMMDD.sqlite` with reason recorded; do
   **not** `UPDATE`-migrate. Re-bench Phi-4-mini with the fixed schema.

**HIGH (fix in the same pass)**

5. **`ep_used` records what was *requested*, not what *ran*.** CoreML fallback
   to CPU is invisible. → Split into `ep_requested` / `ep_actual` /
   `ep_fallback BOOLEAN` / `ep_fallback_reason`. Use a verbose-log probe or
   per-EP try/except to detect actual provider (no stable
   `onnxruntime-genai` API to query EP post-construction as of 2026-04).
6. **Judge has no retry.** Use Anthropic SDK's built-in `max_retries`
   (default 2, bump to 5) and `timeout` — **do not** add `tenacity`;
   SDK-native is the documented path.
7. **Peak-RAM sampler misses prefill.** Prefill is a single
   `append_tokens` call and the RAM peak there is unobserved. → Run a
   background sampler thread at ~50 ms cadence for the whole bench window;
   stop on generation end.
8. **No `model_manifest.json`.** Phi-4-mini revision/hash is not audited.
   → Regenerate manifest as part of re-bench.

**MEDIUM (fix opportunistically)**

9. `aggregate.py` default paths are cwd-relative → anchor to `REPO_ROOT`.
10. ASR engine smoke required before corpus work: `whisperkit-cli` and
    `whisper-cli` are not on PATH in the current env. Install or document.
11. Add unit tests for pure functions before (not after) the schema change:
    `_extract_section`, `_percentile`, `_infer_lang`, `_infer_task_type`.

**LOW (track for Phase 1)**

12. `--label` alone lets humans enter arbitrary strings. Prefer structured
    columns (alias / variant / repo / revision / manifest_hash).
13. ASR corpus source is a user decision, not an autonomous step
    (licensing / privacy). Candidates: self-recorded, CSJ excerpts, public
    domain. Record the decision in the repo before recording.

**Sequenced execution**

```
1. schema + label + EP-provenance + append-only + retry + prompt → code only
2. quarantine results/bench_db.sqlite → bench_db.invalid_20260420.sqlite
3. re-bench Phi-4-mini small (1 JP + 1 EN) → eyeball JP output is Japanese
4. judge smoke on step 3 (5-10 API calls, confirm retry path works)
5. background: download Gemma-4-E2B + Qwen3-4B (with manifest)
6. Tier-1 full bench, full judge, aggregate
7. ASR engine install → corpus decision → ASR bench
8. Day-6/7 gate
```

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

> **Revised 2026-04-21** per ADR-001. The original plan (Python
> `onnxruntime_genai` → Rust `ort` v2 manual KV-cache) hit two blockers in
> Phase 0: (1) `onnxruntime-genai 0.13.1` on macOS has a CoreML file-path
> bug, so all 276 measured runs fell back to CPU; (2) there is no official
> Rust binding for `onnxruntime-genai`, making Phase-1 a 1-2-week spike
> with its own separate CoreML bug surface. The OSS survey
> (`docs/OSS_LANDSCAPE.md`) showed `alan890104/sumi` doing the same flow
> on `candle` + `whisper-rs`, which sidesteps both blockers. See
> `docs/ADR-001-runtime-pivot-candle.md`.

**New Phase-1 runtime**: `candle-core` / `candle-transformers` v0.9.x
(pinned), GGUF models, Metal on macOS, CUDA on Windows x86, CPU on Windows
ARM. ASR unifies on `whisper-rs` across OSes.

### Day 4 — candle spike + re-bench plan

1. Scaffold `research/phase0/candle_spike/` (Rust bin, pinned `candle-*`
   = 0.9.2, Phi-4-mini GGUF load → 1 forward pass → logits shape + TTFT).
2. Download GGUF conversions of Phi-4-mini, Gemma-3-4B, Qwen3-4B, Llama-3.2-3B
   (HF community). Regenerate `model_manifest.json` with `.gguf` entries.
3. Rewrite `bench_llm.py`'s inference core to shell out to a small Rust
   binary, or accept the existing ONNX numbers as reference-only and
   rebuild the bench in Rust. Decision recorded at Day-4 EOD.
4. Re-run the 3-workload smoke (ja_keigo_01 / en_business_01 / jp_en_mix_01)
   on candle for each of the four candidates and verify: (a) load succeeds,
   (b) JP output is Japanese, (c) TTFT is within a 2× band of ONNX CPU
   numbers as a sanity check.
5. Gate the full bench on candle Metal showing ≥ 1.3× speedup over
   ONNX CPU on short workloads — if not, surface as Phase-1 runtime risk.

The Rust `ort` spike at `research/phase0/rust_ort_spike/` is kept for
audit but is **not** the Phase-1 runtime.
