# Phase 0 — PoC benchmarks

Scripts and data for the one-week technical validation described in [docs/PHASE0_POC.md](../../docs/PHASE0_POC.md).

## Layout

```
research/phase0/
├── bench_llm.py          # per-model latency + quality
├── bench_asr.py          # WER on JP/EN/mixed recordings
├── bench_e2e.py          # two-stage vs one-stage pipeline
├── quality_judge.py      # LLM-as-judge with cache
├── models.py             # download + SHA-256 verify
├── runtime_selector.py   # execution-provider picker
├── inputs/               # benchmark corpus (authored text)
│   ├── ja_keigo_01-05.txt
│   ├── jp_en_mix_01-05.txt
│   ├── en_business_01-05.txt
│   └── summary_long_01-03.txt
├── recordings/           # WAV audio (git-ignored)
└── results/
    ├── bench_db.sqlite
    └── report.md
```

The Phase 0 scripts are checked in and currently support model download,
LLM latency/RAM benchmarking, Claude-based judging, ASR CER/WER benchmarking,
and report aggregation.

## Running

```bash
cd research/phase0
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python models.py download 1              # large; see models.py tiers
python bench_llm.py --all
ANTHROPIC_API_KEY=... python quality_judge.py --all
python bench_asr.py --engine whisperkit  # macOS; uses whisperkit-cli
python aggregate.py
cat results/report.md
```

For a cheap smoke pass:

```bash
python models.py smoke phi-4-mini
python bench_llm.py --model phi-4-mini --workload inputs/ja_keigo_01.txt --runs 1 --warmup 0 --max-new-tokens 128
ANTHROPIC_API_KEY=... python quality_judge.py --limit 3
python bench_e2e.py
```

## Hardware

- macOS: Apple Silicon (M1 or later)
- Windows: any x64 machine; Snapdragon X Elite or Intel Core Ultra 7/9 for NPU paths

If NPU hardware isn't available, `bench_llm.py` records a `"pending"` row for the NPU execution provider and continues on CPU / DirectML.

## Budgets

- Disk: ~15 GB (models + recordings)
- Judge calls: capped via on-disk cache; see `quality_judge.py` for the cap
- Time: one person, one week

See the top-level plan for full Go/No-Go criteria.
