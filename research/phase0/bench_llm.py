"""Latency + quality benchmarking for ONNX GenAI models."""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import sqlite3
import sys
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from runtime_selector import detect_platform, select_execution_providers

REPO_ROOT = Path(__file__).resolve().parent
INPUTS_DIR = REPO_ROOT / "inputs"
RESULTS_DIR = REPO_ROOT / "results"
MODELS_DIR = REPO_ROOT / "downloads"
DB_PATH = RESULTS_DIR / "bench_db.sqlite"

DEFAULT_PROMPT_TEMPLATE = (
    "You are rewriting raw dictation into polished prose. "
    "Preserve meaning, remove fillers, keep technical terms verbatim.\n\n"
    "INPUT:\n{input}\n\nREWRITE:\n"
)


@dataclass
class BenchResult:
    model_id: str
    workload_id: str
    ttft_ms: float
    tokens_per_sec: float
    peak_ram_mb: float
    prompt_tokens: int
    completion_tokens: int
    completion_text: str
    input_hash: str
    output_hash: str
    ep_used: str
    run_seq: int
    timestamp: str = field(default_factory=lambda: _dt.datetime.utcnow().isoformat(timespec="seconds"))


_SCHEMA = """
CREATE TABLE IF NOT EXISTS bench_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id        TEXT NOT NULL,
    workload_id     TEXT NOT NULL,
    ttft_ms         REAL NOT NULL,
    tokens_per_sec  REAL NOT NULL,
    peak_ram_mb     REAL NOT NULL,
    prompt_tokens   INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    completion_text TEXT NOT NULL,
    input_hash      TEXT NOT NULL,
    output_hash     TEXT NOT NULL,
    ep_used         TEXT NOT NULL,
    run_seq         INTEGER NOT NULL,
    timestamp       TEXT NOT NULL,
    UNIQUE(model_id, workload_id, run_seq)
);
CREATE INDEX IF NOT EXISTS idx_bench_model_workload
    ON bench_runs(model_id, workload_id);
CREATE INDEX IF NOT EXISTS idx_bench_join
    ON bench_runs(model_id, input_hash, output_hash);
"""


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _peak_rss_mb() -> float:
    import psutil

    return psutil.Process(os.getpid()).memory_info().rss / (1024 * 1024)


def _connect(db_path: Path) -> sqlite3.Connection:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.executescript(_SCHEMA)
    return conn


def store_result(db_path: Path, result: BenchResult) -> None:
    conn = _connect(db_path)
    try:
        conn.execute(
            """
            INSERT OR REPLACE INTO bench_runs
              (model_id, workload_id, ttft_ms, tokens_per_sec, peak_ram_mb,
               prompt_tokens, completion_tokens, completion_text,
               input_hash, output_hash, ep_used, run_seq, timestamp)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                result.model_id,
                result.workload_id,
                result.ttft_ms,
                result.tokens_per_sec,
                result.peak_ram_mb,
                result.prompt_tokens,
                result.completion_tokens,
                result.completion_text,
                result.input_hash,
                result.output_hash,
                result.ep_used,
                result.run_seq,
                result.timestamp,
            ),
        )
        conn.commit()
    finally:
        conn.close()


def _load_workload(path: Path) -> str:
    if not path.exists():
        raise SystemExit(f"workload missing: {path}")
    return path.read_text(encoding="utf-8").strip()


def _render_prompt(workload_text: str) -> str:
    return DEFAULT_PROMPT_TEMPLATE.format(input=workload_text)


def run_bench(
    model_id: str,
    model_dir: Path,
    workload_path: Path,
    runs: int = 5,
    warmup: int = 2,
    max_new_tokens: int = 512,
) -> list[BenchResult]:
    """Run `warmup + runs` generations and return the `runs` measured results."""
    import onnxruntime_genai as og

    ep_tag = ",".join(select_execution_providers()) + f"|{detect_platform()}"

    workload_text = _load_workload(workload_path)
    prompt = _render_prompt(workload_text)
    input_hash = _sha256(workload_text)
    workload_id = workload_path.stem

    model = og.Model(str(model_dir))
    tokenizer = og.Tokenizer(model)

    results: list[BenchResult] = []
    for seq in range(warmup + runs):
        is_warmup = seq < warmup
        input_ids = tokenizer.encode(prompt)

        params = og.GeneratorParams(model)
        params.set_search_options(
            max_length=len(input_ids) + max_new_tokens,
            temperature=0.0,
        )

        generator = og.Generator(model, params)
        generator.append_tokens(input_ids)

        produced: list[int] = []
        peak_ram = _peak_rss_mb()
        t0 = time.perf_counter()
        ttft_ms: float | None = None

        while not generator.is_done() and len(produced) < max_new_tokens:
            generator.generate_next_token()
            if ttft_ms is None:
                ttft_ms = (time.perf_counter() - t0) * 1000.0
            produced.append(int(generator.get_next_tokens()[0]))
            peak_ram = max(peak_ram, _peak_rss_mb())

        elapsed = time.perf_counter() - t0
        if is_warmup:
            continue

        completion = tokenizer.decode(produced)
        result = BenchResult(
            model_id=model_id,
            workload_id=workload_id,
            ttft_ms=ttft_ms or 0.0,
            tokens_per_sec=(len(produced) / elapsed) if elapsed > 0 else 0.0,
            peak_ram_mb=peak_ram,
            prompt_tokens=len(input_ids),
            completion_tokens=len(produced),
            completion_text=completion,
            input_hash=input_hash,
            output_hash=_sha256(completion),
            ep_used=ep_tag,
            run_seq=seq - warmup,
        )
        results.append(result)

    return results


def _iter_workloads(paths: list[Path]) -> list[Path]:
    out: list[Path] = []
    for p in paths:
        if p.is_dir():
            out.extend(sorted(p.glob("*.txt")))
        else:
            out.append(p)
    return out


def _tier_models(tier: str) -> dict[str, Path]:
    from models import TIER_1, TIER_2

    aliases = {"1": TIER_1, "2": TIER_2, "all": {**TIER_1, **TIER_2}}[tier]
    return {alias: MODELS_DIR / alias for alias in aliases}


def _cli() -> None:
    parser = argparse.ArgumentParser(description="Per-model latency + quality bench.")
    parser.add_argument("--model", help="alias (see models.py) or path to model dir")
    parser.add_argument("--tier", choices=["1", "2", "all"])
    parser.add_argument("--workload", type=Path, help="single .txt file")
    parser.add_argument("--workloads-dir", type=Path, default=INPUTS_DIR)
    parser.add_argument("--all", action="store_true", help="Tier 1 x all workloads")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--max-new-tokens", type=int, default=512)
    parser.add_argument("--db", type=Path, default=DB_PATH)
    args = parser.parse_args()

    if args.all:
        model_map = _tier_models("1")
        workloads = _iter_workloads([args.workloads_dir])
    elif args.tier:
        model_map = _tier_models(args.tier)
        workloads = _iter_workloads([args.workload or args.workloads_dir])
    else:
        if not args.model:
            raise SystemExit("--model or --tier or --all required")
        from models import ALL_MODELS

        if args.model in ALL_MODELS:
            model_map = {args.model: MODELS_DIR / args.model}
        else:
            p = Path(args.model)
            if not p.is_dir():
                raise SystemExit(f"unknown model: {args.model}")
            model_map = {p.name: p}
        workloads = _iter_workloads([args.workload or args.workloads_dir])

    if not workloads:
        raise SystemExit("no workloads found")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    for alias, model_dir in model_map.items():
        if not model_dir.exists():
            print(f"skip (not downloaded): {alias}", file=sys.stderr)
            continue
        for wp in workloads:
            print(f"bench: {alias} x {wp.name}")
            results = run_bench(
                model_id=alias,
                model_dir=model_dir,
                workload_path=wp,
                runs=args.runs,
                warmup=args.warmup,
                max_new_tokens=args.max_new_tokens,
            )
            for r in results:
                store_result(args.db, r)
                print(
                    f"  run={r.run_seq} ttft={r.ttft_ms:.1f}ms "
                    f"tok/s={r.tokens_per_sec:.1f} ram={r.peak_ram_mb:.0f}MB"
                )
            summary = {
                "model_id": alias,
                "workload_id": wp.stem,
                "runs": [asdict(r) for r in results],
            }
            print(json.dumps({"summary": {k: v for k, v in summary.items() if k != "runs"}}))


if __name__ == "__main__":
    _cli()
