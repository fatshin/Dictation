# Phase 0 aggregated report

## Hard lines

- TTFT p95 < 2500 ms (target < 1500 ms)
- LLM-as-judge quality >= 7.0
- ASR CER avg < 10 %
- Peak RAM < 8192 MB

## ASR

- Engine: whisperkit
- Platform: macos-arm64
- Utterances: 6
- CER avg: 24.19 % FAIL
- WER avg: 83.77 %

## Per-model summary

| Model | Runs | Judged | TTFT p50 (ms) | TTFT p95 (ms) | tok/s p50 | Quality avg | Peak RAM (MB) | Verdict |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| llama-3.2-3b-genai | 92 | 0 | 2445 | 12858 | 18.8 | UNJUDGED | 4863 | **BLOCKED** |
| phi-4-mini | 92 | 0 | 2851 | 17934 | 15.6 | UNJUDGED | 6121 | **BLOCKED** |
| phi-3.5-mini | 92 | 0 | 7376 | 42189 | 10.3 | UNJUDGED | 9850 | **BLOCKED** |

Models passing per-model hard lines: 0
ASR hard line: FAIL

Phase 0 gate NOT cleared: judge coverage incomplete.
Phase 0 gate NOT cleared: ASR CER above hard line (or report missing).
Phase 0 gate NOT cleared: fewer than two models passing per-model hard lines.