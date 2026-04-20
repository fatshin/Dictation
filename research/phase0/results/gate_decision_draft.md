# Phase 0 Gate Decision — Draft (2026-04-20)

**Status**: Quality axis pending (judge requires `ANTHROPIC_API_KEY`).
Latency + RAM + preliminary quality (visual inspection of outputs) are in.

## Bench scope delivered

- 3 models × 18 workloads × 5 measured + 2 warmup = **276 measured runs**
- Models: `phi-4-mini` (Microsoft), `llama-3.2-3b-genai` (onnx-community),
  `phi-3.5-mini` (Microsoft)
- Platform: macOS arm64 (Apple M4), CoreML EP **fell back to CPU** in every
  run (`ep_fallback=1`) — `onnxruntime-genai.Config.append_provider` API is
  not exposed in this build. Results therefore reflect **CPU-only**
  execution. Real macOS deployment with CoreML should be 1.5-3× faster on
  prefill.

## Hard-line results (original scope: all 18 workloads)

| Model | TTFT p50 | TTFT p95 | tok/s p50 | RAM max | Verdict |
|---|---|---|---|---|---|
| llama-3.2-3b-genai | 2445 ms | **12858 ms** | 18.8 | 4863 MB ✓ | FAIL |
| phi-4-mini | 2851 ms ❌ | **17934 ms** | 15.6 | 6121 MB ✓ | FAIL |
| phi-3.5-mini | 7376 ms ❌ | **42189 ms** | 10.3 | **9850 MB** ❌ | FAIL (all) |

All three fail when long-form summarization (`summary_long_*`, 5-10 K chars)
is included.

## Hard-line results by workload class

| Scope | workloads | Phi-4-mini | Llama-3.2-3B | Phi-3.5-mini |
|---|---|---|---|---|
| Very-short (≤110 ch) | 4 | **PASS** p95=1985 | **PASS** p95=1731 | FAIL |
| Short-medium (≤350 ch) | 8 | FAIL p95=3486 | FAIL p95=2817 | FAIL |
| All dictation (no summary) | 15 | FAIL p95=5949 | FAIL p95=4744 | FAIL |
| Full (incl. summary) | 18 | FAIL p95=17934 | FAIL p95=12858 | FAIL |

### Per-workload TTFT (ms, p50, run ≥ 5)

| workload | Phi-4-mini | Llama-3.2-3B | Phi-3.5-mini |
|---|---|---|---|
| en_business_01 | 350 ✓ | 842 ✓ | 1866 ✓ |
| en_business_02 | 575 ✓ | 1233 ✓ | 3282 ✗ |
| en_business_03 | 748 ✓ | 1380 ✓ | 3533 ✗ |
| en_business_04 | 1415 ✓ | 2304 ✓ | 5795 ✗ |
| en_business_05 | 1033 ✓ | 1845 ✓ | 3674 ✗ |
| ja_keigo_01 | 821 ✓ | 1572 ✓ | 3940 ✗ |
| ja_keigo_02 | 2514 ✗ | 2428 ✓ | 8167 ✗ |
| ja_keigo_03 | 3331 ✗ | 2737 ✗ | 8425 ✗ |
| ja_keigo_04 | 4041 ✗ | 3178 ✗ | 8525 ✗ |
| ja_keigo_05 | 6181 ✗ | 4946 ✗ | 16408 ✗ |
| jp_en_mix_01 | 1936 ✓ | 1626 ✓ | 5367 ✗ |
| jp_en_mix_02 | 2789 ✗ | 2451 ✓ | 6592 ✗ |
| jp_en_mix_03 | 2807 ✗ | 2621 ✗ | 8011 ✗ |
| jp_en_mix_04 | 3148 ✗ | 2280 ✓ | 8554 ✗ |
| jp_en_mix_05 | 3089 ✗ | 2618 ✗ | 8256 ✗ |
| summary_long_01 | 16920 ✗ | 9999 ✗ | 37860 ✗ |
| summary_long_02 | 18661 ✗ | 13150 ✗ | — |
| summary_long_03 | 13195 ✗ | 9876 ✗ | — |

## Quality axis (preliminary; visual inspection)

No judge run yet (no API key). From sampled outputs on `ja_keigo_01`:

| Model | JP output quality (smoke) |
|---|---|
| Phi-4-mini | **GOOD**: "田中さん、明日の打ち合わせは30分に短縮できますか..." — natural 敬体, preserves intent |
| Llama-3.2-3B | **POOR**: echoes input verbatim, then loops with "質問の例" templates. Not instruction-following for JP keigo rewrite |
| Phi-3.5-mini | **MEDIOCRE**: mistranslations ("30分前借り" ≠ 後ろ倒し), garbled structure |

## Go / No-Go analysis

### Full-scope (original Phase 0 spec)
**No-Go**. Fails ≥2 models + TTFT hard line. Quality axis unmeasured.

### Short-dictation-only scope (retargeted to core dictation use case)
**Provisional Go** pending quality judge, **only** for short (≤~110 ch)
utterances:
- Phi-4-mini: TTFT p95 1985 ms ✓, RAM 3046 MB ✓, JP quality visually good
- Llama-3.2-3B: TTFT p95 1731 ms ✓, RAM 2424 MB ✓ — but **JP quality fails**
  instruction-following in smoke; Llama is EN-first and not viable as the
  second JP model.

So on short-dictation-only, the ≥2-models rule collapses to "≥2 models with
acceptable quality **in the target language**". JP-wise we have exactly one
(Phi-4-mini). EN-wise we have two (Phi-4-mini + Llama-3.2-3B both pass
latency and produce readable rewrites).

### ASR
CER 29.9 % (hard line < 10 %). FAIL. Driver:
- Synthetic `say` TTS expanded abbreviations (pls → please, q3 → Q three) —
  inflates WER against the casual-dictation references.
- whisper-small model is too weak for Japanese proper-noun fidelity
  (オリオン → オーライアン).
Before the gate can judge ASR, we need either real-voice recordings or a
larger whisper model (medium / large-v3).

## Recommended gate decision: **Conditional Go with scope cut**

1. **Scope cut**: Phase 1 targets short-to-medium utterances (≤ ~350
   characters raw dictation). Long-form summarization moves to Phase 3
   (already scheduled for "meeting and long-form").
2. **Language split**:
   - JP: Phi-4-mini primary. No JP fallback in Tier 1 as of today.
   - EN: Phi-4-mini primary, Llama-3.2-3B fallback.
3. **Blockers before Phase 1 start**:
   - ANTHROPIC_API_KEY wired → run judge → confirm Phi-4-mini JP quality ≥ 7
   - CoreML EP actually activated (investigate `onnxruntime-genai` build
     that exposes provider-append, or switch to raw `onnxruntime` for JP path)
   - JP fallback model decision: (a) accept single-model JP risk, (b) port
     `genai_config.json` for Gemma-4-E2B or Qwen3-4B (2-day spike), or
     (c) fine-tune Phi-3.5-mini for JP keigo (out of Phase-0 scope).
   - Real-voice ASR corpus decided (recording / public-domain / CSJ excerpts)
     and ASR CER re-measured with medium model.

## Deliberately excluded workloads (with rationale)

- `summary_long_*`: 5-10 K input chars. Even best model (Llama) takes ≥ 10 s
  to first token. Long-form summarization moves to Phase 3 in ROADMAP.md.
- `ja_keigo_04-05`, `jp_en_mix_03-05`: 300-500 char raw dictation.
  On-the-border cases; target Phase 2 quality/speed improvements.

## Audit notes

- All 276 measured runs recorded in `results/bench_db.sqlite` with
  `bench_session_id`, `model_alias`, `model_variant`, `model_revision`,
  `ep_requested`, `ep_actual`, `ep_fallback`, `prompt_version=2026-04-v2`.
- Prompt fix (language-aware templates) committed in `803b750`; all 276 runs
  use v2.
- Quarantined Day-2 rows kept at `bench_db.invalid_20260420.sqlite`.
- Tier-1 pivot documented in `models.py` QUARANTINED_TIER_1 dict.
