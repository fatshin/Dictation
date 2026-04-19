# Phase 0 aggregated report

## Hard lines

- TTFT p95 < 2500 ms (target < 1500 ms)
- LLM-as-judge quality >= 7.0
- ASR CER avg < 10 %
- Peak RAM < 8192 MB

## ASR

**No ASR report found.** Run `bench_asr.py` before the Phase 0 gate.

## Per-model summary

| Model | Runs | TTFT p50 (ms) | TTFT p95 (ms) | tok/s p50 | Quality avg | Peak RAM (MB) | Verdict |
|---|---|---|---|---|---|---|---|
| cpu-int4-rtn-block-32-acc-level-4 | 10 | 752 | 1851 | 16.3 | 0.00 | 2985 | **FAIL** |

Models passing per-model hard lines: 0
ASR hard line: FAIL

Phase 0 gate NOT cleared: ASR CER above hard line (or report missing).
Phase 0 gate NOT cleared: fewer than two models passing per-model hard lines.