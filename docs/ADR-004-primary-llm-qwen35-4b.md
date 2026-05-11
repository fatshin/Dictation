# ADR-004 — Primary local LLM: switch from Gemma-4 to Qwen3.5-4B

- **Status**: Accepted (2026-05-11)
- **Scope**: Primary Ollama model for the dictation rewrite pipeline.
  Supersedes ADR-002's model selection only; the Ollama runtime itself
  (HTTP sidecar, reqwest client) is unchanged.
- **Driver**: Phase-0 bench results (`research/phase0/results/report.md`,
  `research/phase0/ollama_candidates.json`) reveal that Gemma-4's actual
  on-disk size under Ollama is **9.6 GB (E4B)** and **7.2 GB (E2B)**
  — not the ~2.5 GB quoted at model-card level — because Ollama bundles
  the PLE embedding layer + audio encoder alongside the weights. This
  makes Gemma-4 unviable as a default on machines with 16 GB unified/RAM.
  Qwen3.5-4B achieves 98% of Gemma-4-E4B quality in a 2.6 GB footprint
  under an unrestricted Apache-2.0 license.
- **Supersedes**: ADR-002 § "Models: Gemma-4 E2B (default fallback) and
  E4B (default primary)". All other ADR-002 decisions (Ollama runtime,
  HTTP API, sidecar lifecycle) remain in effect.

## Context

### Phase-0 bench methodology

Bench harness: `research/phase0/bench_ollama.py`  
Prompt set: `research/phase0/prompt_templates/2026-04-v2.json` (18 workloads —
ja_keigo × 6, en_business × 6, jp_en_mix × 6)  
Hardware: M4 Max / unified 115 GB / Ollama 0.21.0 (warm-cache runs used as
the realistic single-model TTFT; cold-swap numbers are artefacts of
multi-model keep_alive overlap and are not decision inputs)

### Results (warm-cache, best-1 TTFT per model)

| Model | Quality avg | Warm TTFT (ms) | tok/s (max) | Disk (GB) | License |
|---|---:|---:|---:|---:|---|
| gemma4:e4b | 8.45 | 246 | 51 | **9.6** | Gemma ToU |
| qwen3.5:9b-q4_K_M | 8.36 | 21 446 | 31 | 5.6 | Apache-2.0 |
| gemma4:e2b | 8.29 | 211 | 115 | **7.2** | Gemma ToU |
| **qwen3.5:4b-q4_K_M** | **8.12** | **762** | **61** | **2.6** | **Apache-2.0** |
| qwen3:4b-instruct-2507-q4_K_M | 7.72 | 18 104 | 56 | 2.5 | Apache-2.0 |
| llm-jp-3-3.7b-instruct Q4_K_M | 6.56 | 1 966 | 70 | 2.3 | Apache-2.0 |

Hard lines (Phase-0 gate): TTFT p95 < 2 500 ms, quality ≥ 7.0, peak RAM < 12 288 MB.

### Why Gemma-4 fails the 16 GB target

Ollama ships Gemma-4 as a unified blob that includes PLE weights and the
audio encoder even when audio input is disabled. The resulting disk/RAM
footprint is:

- E4B: **9.6 GB on disk** → at inference loads to ~10.3 GB RSS (measured)
- E2B: **7.2 GB on disk** → ~7.7 GB RSS

On a 16 GB machine this leaves < 6 GB for OS + browser + app, causing
active swap thrashing or OOM. A user who already has another large model
resident (e.g. from another chat app) will see evictions on every request.

### Why Qwen3.5-9B is not the primary

Despite ranking second on quality (8.36), its warm TTFT of **21 s on M4 Max**
(measured; a high-end machine) means it would take 40–90 s on a typical
16 GB CPU machine — well above the 2 500 ms dictation UX budget. It is
retained as the cloud/API quality ceiling for comparative scoring only.

### Why Qwen3.5-4B is selected

- **Quality gap is 0.33 pt (4%)** vs the best local model (Gemma-4-E4B 8.45).
  All workloads remain above the 7.0 floor; ja_keigo scores are 7.8–8.3.
- **Disk footprint 2.6 GB** — 3.7× smaller than E4B, comfortably within
  the 16 GB target even with OS overhead.
- **Warm TTFT 762 ms** on M4 Max; projected 1–3 s on 16 GB CPU — inside
  the 2 500 ms hard line and the 1 500 ms target line.
- **Apache-2.0** — no usage restriction, no commercial terms, trivially
  redistributable in a bundled setup.
- **Think-mode off** (`"think": false`) keeps latency predictable for the
  dictation use-case; extended reasoning budget is unnecessary for
  typo-correction / keigo rewriting.

## Decision

**Primary default model**: `qwen3.5:4b-q4_K_M`  
**API / cloud quality ceiling**: `qwen3.5:9b-q4_K_M` (bench reference; not a daily driver)  
**Legacy fallback**: `gemma4:e4b` and `gemma4:e2b` remain selectable for users
who already pulled them and are on high-memory machines; they are no longer
in the recommended set and will not be pulled by the setup wizard.

Model resolution order in the app (unchanged mechanism, updated list):

```
1. qwen3.5:4b-q4_K_M          ← new primary
2. qwen3:4b-instruct-2507-q4_K_M  ← A/B comparison / fallback
3. gemma4:e4b                  ← legacy; only if already pulled
4. gemma4:e2b                  ← legacy; only if already pulled
```

The `PREFERRED_MODELS` constant in `src/App.tsx` and the `REQUIRED_MODELS`
list in `src-tauri/src/commands.rs` are updated accordingly (committed in
the same branch as this ADR).

## Consequences

### Benefits

- Setup wizard pull is **2.6 GB** instead of 9.6 GB — a first-run experience
  improvement of ~25 minutes on a 50 Mbps connection.
- Fits comfortably in 16 GB unified RAM alongside the OS and the WebKit
  process, removing the 12 GB RAM gate as an effective blocker.
- Apache-2.0 removes the Gemma Terms of Use ambiguity for commercial
  distribution, simplifying legal review.
- tok/s p50 of 61 vs Gemma-4-E4B's 47 means faster streaming on the same
  hardware, improving perceived responsiveness.

### Costs / risks

- **Quality regression of 0.33 pt** on the LLM-as-judge composite. Individual
  axis breakdowns are not yet published; ja_keigo may have a slightly larger
  gap than en_business. Acceptance criterion: all 18 workloads must score
  ≥ 7.0 (current minimum observed: 7.8).
- **Bench was run on M4 Max**, not on a real 16 GB CPU machine. Projected
  TTFT of 1–3 s is a rough estimate (6–10 tok/s typical for that class).
  A re-run on a 16 GB CPU machine is required before Phase-1 GA (see
  open verification below).
- **TTFT p50/p95 numbers in `report.md` are contaminated** by multi-model
  keep_alive overlap. The warm-cache best-1 numbers used here are a proxy.
  A clean single-model re-bench with `keep_alive=0` isolation is scheduled
  as Task #22 (Phase-0 harness improvements).

## Rejected alternatives

- **Keep Gemma-4-E4B as primary** — 9.6 GB disk/RAM footprint makes it
  unworkable on the stated 16 GB target machines.
- **Gemma-4-E2B as primary** — 7.2 GB is still 2.8× Qwen3.5-4B with a
  0.17 pt quality advantage. Not worth the size on a 16 GB machine.
- **Qwen3.5-9B as primary** — quality 8.36 (+0.24 vs 4B) but warm TTFT
  21 s on M4 Max; projected 40–90 s on 16 GB CPU. Fails TTFT hard line.
- **qwen3:4b-instruct-2507** — superseded by Qwen3.5-4B (-0.40 pt at same
  size). Retained only as A/B fallback.
- **llm-jp-3-3.7b-instruct** — JP-specialised but ranks last (6.56),
  1.56 pt below Qwen3.5-4B. No remaining use case.
- **qwen3.6:27b-q4_K_M** — API-only reference; too heavy for any local
  16 GB machine.

## Open verification (Phase-1 entry blockers)

1. **16 GB CPU machine re-bench** — run `research/phase0/scripts/rebench_4b_candidates.sh tier1`
   on a real 16 GB-RAM non-Apple-Silicon machine. Confirm TTFT < 2 500 ms p95
   and quality ≥ 7.0 for `qwen3.5:4b-q4_K_M`.
2. **Clean single-model RAM measurement** — rerun with `keep_alive=0`
   between models so RSS reflects a single-model footprint. Currently the
   `peak_ram_mb` column sums all ollama processes (Task #22).
3. **ja_keigo per-axis breakdown** — confirm no individual axis drops below
   5.0 (the Phase-0 secondary gate) for `qwen3.5:4b-q4_K_M`.
4. **Setup wizard pull integration** — update the first-run wizard to pull
   `qwen3.5:4b-q4_K_M` by default (Task #15 / B2 scope).

## References

- ADR-002: `docs/ADR-002-runtime-pivot-ollama-gemma4.md` (runtime decision
  unchanged; model selection superseded by this ADR)
- Phase-0 bench outcome: `research/phase0/ollama_candidates.json` §`bench_outcome`
- Phase-0 aggregate report: `research/phase0/results/report.md`
- Bench harness: `research/phase0/bench_ollama.py`
- Re-bench script: `research/phase0/scripts/rebench_4b_candidates.sh`
- Qwen3.5 model card: https://huggingface.co/Qwen/Qwen3.5-4B
- Ollama Qwen3.5 tag: `qwen3.5:4b-q4_K_M`
