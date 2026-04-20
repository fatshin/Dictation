# Phase 0 aggregated report

## Hard lines

- TTFT p95 < 2500 ms (target < 1500 ms)
- LLM-as-judge quality >= 7.0
- ASR CER avg < 10 %
- Peak RAM < 8192 MB

## ASR

- Engine: whisper.cpp
- Platform: macos-arm64
- Utterances: 6
- CER avg: 29.93 % FAIL
- WER avg: 84.68 %

## Per-model summary

| Model | Runs | TTFT p50 (ms) | TTFT p95 (ms) | tok/s p50 | Quality avg | Peak RAM (MB) | Verdict |
|---|---|---|---|---|---|---|---|
| llama-3.2-3b-genai | 92 | 2445 | 12858 | 18.8 | 0.00 | 4863 | **FAIL** |
| phi-4-mini | 92 | 2851 | 17934 | 15.6 | 0.00 | 6121 | **FAIL** |
| phi-3.5-mini | 92 | 7376 | 42189 | 10.3 | 0.00 | 9850 | **FAIL** |

Models passing per-model hard lines: 0
ASR hard line: FAIL

Phase 0 gate NOT cleared: ASR CER above hard line (or report missing).
Phase 0 gate NOT cleared: fewer than two models passing per-model hard lines.