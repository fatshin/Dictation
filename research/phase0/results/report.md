# Phase 0 aggregated report

## Hard lines

- TTFT p95 < 2500 ms (target < 1500 ms)
- LLM-as-judge quality >= 7.0
- ASR CER avg < 10 %
- Peak RAM < 12288 MB

## ASR

- Engine: whisperkit
- Platform: macos-arm64
- Utterances: 6
- CER avg: 24.19 % FAIL
- WER avg: 83.77 %

## Per-model summary

| Model | Runs | Judged | TTFT p50 (ms) | TTFT p95 (ms) | tok/s p50 | Quality avg | Peak RAM (MB) | Verdict |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| gemma4-e4b | 90 | 90 | 371 | 1141 | 47.2 | 8.45 | 10300 | **PASS** |
| gemma4-e2b | 90 | 90 | 260 | 311 | 66.3 | 8.29 | 7700 | **FAIL** |

Models passing per-model hard lines: 1
ASR hard line: FAIL

Phase 0 gate NOT cleared: ASR CER above hard line (or report missing).
Phase 0 gate NOT cleared: fewer than two models passing per-model hard lines.