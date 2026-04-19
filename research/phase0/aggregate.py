from __future__ import annotations

import argparse
import json
import sqlite3
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path

HARD_LINE_TTFT_MS = 2500.0
TARGET_TTFT_MS = 1500.0
HARD_LINE_QUALITY = 7.0
HARD_LINE_CER_PCT = 10.0
HARD_LINE_RAM_MB = 8 * 1024.0


@dataclass
class ModelSummary:
    model_id: str
    runs: int
    ttft_ms_median: float
    ttft_ms_p95: float
    tokens_per_sec_median: float
    quality_avg: float
    peak_ram_mb_max: float
    pass_ttft: bool
    pass_quality: bool
    pass_ram: bool
    verdict: str


def load_bench(db_path: Path) -> list[dict]:
    if not db_path.exists():
        return []
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    rows = con.execute("SELECT * FROM bench_result").fetchall()
    con.close()
    return [dict(r) for r in rows]


def load_judge(db_path: Path) -> dict[tuple[str, str], dict]:
    if not db_path.exists():
        return {}
    con = sqlite3.connect(db_path)
    con.row_factory = sqlite3.Row
    rows = con.execute("SELECT * FROM judge_cache").fetchall()
    con.close()
    out: dict[tuple[str, str], dict] = {}
    for r in rows:
        key = (r["model_id"], r["output_hash"])
        out[key] = dict(r)
    return out


def summarize(results: list[dict], judge: dict[tuple[str, str], dict]) -> list[ModelSummary]:
    by_model: dict[str, list[dict]] = {}
    for r in results:
        by_model.setdefault(r["model_id"], []).append(r)

    summaries: list[ModelSummary] = []
    for model_id, rs in by_model.items():
        ttfts = [r["ttft_ms"] for r in rs if r.get("ttft_ms") is not None]
        tps = [r["tokens_per_sec"] for r in rs if r.get("tokens_per_sec")]
        rams = [r["peak_ram_mb"] for r in rs if r.get("peak_ram_mb")]
        qualities: list[float] = []
        for r in rs:
            j = judge.get((model_id, r.get("output_hash", "")))
            if j:
                axes = [j.get(k) for k in ("keigo", "filler", "semantic", "structure")]
                axes = [a for a in axes if a is not None]
                if axes:
                    qualities.append(statistics.mean(axes))

        ttft_med = statistics.median(ttfts) if ttfts else float("inf")
        ttft_p95 = _percentile(ttfts, 0.95) if ttfts else float("inf")
        tps_med = statistics.median(tps) if tps else 0.0
        ram_max = max(rams) if rams else float("inf")
        quality_avg = statistics.mean(qualities) if qualities else 0.0

        pass_ttft = ttft_p95 < HARD_LINE_TTFT_MS
        pass_quality = quality_avg >= HARD_LINE_QUALITY
        pass_ram = ram_max < HARD_LINE_RAM_MB
        verdict = "PASS" if (pass_ttft and pass_quality and pass_ram) else "FAIL"

        summaries.append(ModelSummary(
            model_id=model_id,
            runs=len(rs),
            ttft_ms_median=ttft_med,
            ttft_ms_p95=ttft_p95,
            tokens_per_sec_median=tps_med,
            quality_avg=quality_avg,
            peak_ram_mb_max=ram_max,
            pass_ttft=pass_ttft,
            pass_quality=pass_quality,
            pass_ram=pass_ram,
            verdict=verdict,
        ))
    summaries.sort(key=lambda s: (s.verdict != "PASS", -s.quality_avg, s.ttft_ms_p95))
    return summaries


def _percentile(data: list[float], pct: float) -> float:
    if not data:
        return float("inf")
    ordered = sorted(data)
    k = (len(ordered) - 1) * pct
    lo = int(k)
    hi = min(lo + 1, len(ordered) - 1)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (k - lo)


def render_markdown(summaries: list[ModelSummary]) -> str:
    lines = [
        "# Phase 0 aggregated report",
        "",
        "## Hard lines",
        "",
        f"- TTFT p95 < {HARD_LINE_TTFT_MS:.0f} ms (target < {TARGET_TTFT_MS:.0f} ms)",
        f"- LLM-as-judge quality >= {HARD_LINE_QUALITY}",
        f"- Peak RAM < {HARD_LINE_RAM_MB:.0f} MB",
        "",
        "## Per-model summary",
        "",
        "| Model | Runs | TTFT p50 (ms) | TTFT p95 (ms) | tok/s p50 | Quality avg | Peak RAM (MB) | Verdict |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for s in summaries:
        lines.append(
            f"| {s.model_id} | {s.runs} | {s.ttft_ms_median:.0f} | {s.ttft_ms_p95:.0f} "
            f"| {s.tokens_per_sec_median:.1f} | {s.quality_avg:.2f} "
            f"| {s.peak_ram_mb_max:.0f} | **{s.verdict}** |"
        )
    lines.append("")
    passes = [s for s in summaries if s.verdict == "PASS"]
    lines.append(f"Models passing all hard lines: {len(passes)}")
    if len(passes) >= 2:
        lines.append("")
        lines.append(f"Recommended primary: {passes[0].model_id}")
        lines.append(f"Recommended fallback: {passes[1].model_id}")
    else:
        lines.append("")
        lines.append("Phase 0 gate NOT cleared. Fewer than two models passing all hard lines.")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Aggregate Phase 0 bench + judge results")
    parser.add_argument("--bench-db", default="results/bench_db.sqlite")
    parser.add_argument("--judge-db", default="results/judge_cache.sqlite")
    parser.add_argument("--out", default="results/report.md")
    args = parser.parse_args()

    results = load_bench(Path(args.bench_db))
    judge = load_judge(Path(args.judge_db))

    if not results:
        print("No bench results yet — nothing to aggregate", file=sys.stderr)
        return 1

    summaries = summarize(results, judge)
    md = render_markdown(summaries)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(md)
    print(md)
    print(f"\nWrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
