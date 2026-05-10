"""Phase-0 bench harness for Ollama-served LLMs (ADR-002 pivot to Gemma-4).

Schema-compatible with bench_llm.py. Reuses prompt_templates/<version>.json
as the single source of prompts (Codex review, Day-4.5).

TTFT defined as request-sent → first non-empty SSE chunk, matching the
candle-bench definition (prefill→first-token-sampled in spirit; HTTP
overhead is recorded as part of the user-perceived latency, which is the
right thing for the dictation use case).
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import sqlite3
import sys
import threading
import time
import urllib.request
import uuid
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent
INPUTS_DIR = REPO_ROOT / "inputs"
RESULTS_DIR = REPO_ROOT / "results"
DB_PATH = RESULTS_DIR / "bench_db.sqlite"
PROMPTS_PATH = REPO_ROOT / "prompt_templates" / "2026-04-v2.json"

OLLAMA_HOST = "http://127.0.0.1:11434"


@dataclass
class BenchResult:
    bench_session_id: str
    model_alias: str
    model_variant: str
    model_repo: str
    model_revision: str
    workload_id: str
    ttft_ms: float
    tokens_per_sec: float
    peak_ram_mb: float
    prompt_tokens: int
    completion_tokens: int
    completion_text: str
    input_hash: str
    output_hash: str
    ep_requested: str
    ep_actual: str
    ep_fallback: int
    ep_fallback_reason: str
    run_seq: int
    platform_tag: str
    prompt_version: str
    timestamp: str = field(default_factory=lambda: _dt.datetime.now(_dt.UTC).isoformat(timespec="seconds"))


_SCHEMA = """
CREATE TABLE IF NOT EXISTS bench_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    bench_session_id    TEXT NOT NULL,
    model_alias         TEXT NOT NULL,
    model_variant       TEXT NOT NULL,
    model_repo          TEXT NOT NULL,
    model_revision      TEXT NOT NULL,
    workload_id         TEXT NOT NULL,
    ttft_ms             REAL NOT NULL,
    tokens_per_sec      REAL NOT NULL,
    peak_ram_mb         REAL NOT NULL,
    prompt_tokens       INTEGER NOT NULL,
    completion_tokens   INTEGER NOT NULL,
    completion_text     TEXT NOT NULL,
    input_hash          TEXT NOT NULL,
    output_hash         TEXT NOT NULL,
    ep_requested        TEXT NOT NULL,
    ep_actual           TEXT NOT NULL,
    ep_fallback         INTEGER NOT NULL,
    ep_fallback_reason  TEXT NOT NULL,
    run_seq             INTEGER NOT NULL,
    platform_tag        TEXT NOT NULL,
    prompt_version      TEXT NOT NULL,
    timestamp           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bench_alias_workload
    ON bench_runs(model_alias, workload_id);
CREATE INDEX IF NOT EXISTS idx_bench_join
    ON bench_runs(model_alias, input_hash, output_hash);
CREATE INDEX IF NOT EXISTS idx_bench_session
    ON bench_runs(bench_session_id);
"""


def _sha256(s: str) -> str:
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


def _connect(p: Path) -> sqlite3.Connection:
    p.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(p)
    conn.executescript(
        "PRAGMA journal_mode=WAL;"
        "PRAGMA synchronous=NORMAL;"
        "PRAGMA busy_timeout=5000;"
    )
    conn.executescript(_SCHEMA)
    return conn


def _peak_rss_mb() -> float:
    import os
    import psutil

    proc = psutil.Process(os.getpid())
    total = proc.memory_info().rss
    for child in proc.children(recursive=True):
        try:
            total += child.memory_info().rss
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return total / (1024 * 1024)


def _ollama_proc_rss_mb() -> float:
    """RSS of the ollama serve process + spawned model runner. Bench client RSS
    is irrelevant; the LLM lives in the daemon."""
    import psutil

    total = 0.0
    for p in psutil.process_iter(attrs=["name", "memory_info"]):
        try:
            n = (p.info.get("name") or "").lower()
            if "ollama" not in n:
                continue
            mem = p.info.get("memory_info")
            # macOS occasionally returns None when the process is being
            # spawned/torn down between the iter() snapshot and our access.
            # Treat as 0 rather than crashing the whole bench.
            if mem is None:
                continue
            total += mem.rss
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return total / (1024 * 1024)


class _RAMSampler:
    """Sample RSS of the ollama daemon (not the bench python process)."""

    def __init__(self, interval_sec: float = 0.05) -> None:
        self._interval = interval_sec
        self._peak = 0.0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._peak = _ollama_proc_rss_mb()
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                self._peak = max(self._peak, _ollama_proc_rss_mb())
            except Exception:
                pass
            self._stop.wait(self._interval)

    def stop(self) -> float:
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=1.0)
        self._peak = max(self._peak, _ollama_proc_rss_mb())
        return self._peak


def _extract_input(text: str) -> str:
    marker = "## INPUT"
    start = text.index(marker) + len(marker)
    rest = text[start:]
    end = rest.find("\n## ")
    return (rest[:end] if end >= 0 else rest).strip()


def _task_type(workload_id: str) -> str:
    for prefix in ("ja_keigo", "jp_en_mix", "en_business", "summary"):
        if workload_id.startswith(prefix):
            return prefix
    raise SystemExit(f"unknown workload prefix: {workload_id}")


def _ollama_generate(model: str, prompt: str, max_new: int) -> dict:
    """One streaming call. Returns dict with ttft_ms, decode_ms, response,
    eval_count, prompt_eval_count."""
    payload = json.dumps({
        "model": model,
        "prompt": prompt,
        "stream": True,
        "think": False,
        "options": {"temperature": 0.0, "num_predict": max_new},
    }).encode()
    req = urllib.request.Request(
        f"{OLLAMA_HOST}/api/generate",
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    chunks: list[str] = []
    t0 = time.perf_counter()
    ttft_ms: float | None = None
    last_event: dict = {}
    with urllib.request.urlopen(req, timeout=600) as r:
        for line in r:
            d = json.loads(line)
            tok = d.get("response", "")
            if tok and ttft_ms is None:
                ttft_ms = (time.perf_counter() - t0) * 1000.0
            chunks.append(tok)
            if d.get("done"):
                last_event = d
                break
    elapsed = time.perf_counter() - t0
    return {
        "response": "".join(chunks),
        "ttft_ms": ttft_ms or 0.0,
        "elapsed_sec": elapsed,
        "eval_count": last_event.get("eval_count", 0),
        "prompt_eval_count": last_event.get("prompt_eval_count", 0),
        "eval_duration_ns": last_event.get("eval_duration", 0),
        "done_reason": last_event.get("done_reason", ""),
    }


def run_bench(
    model_alias: str,
    ollama_model: str,
    workloads_dir: Path,
    prompts_path: Path,
    runs: int,
    warmup: int,
    max_new_tokens: int,
    db_path: Path,
    only: list[str] | None,
) -> int:
    session_id = uuid.uuid4().hex
    print(f"bench_session_id={session_id}")

    prompts = json.loads(prompts_path.read_text(encoding="utf-8"))
    prompt_version = prompts["version"]
    templates = prompts["templates"]

    workloads = sorted(workloads_dir.glob("*.txt"))
    if only:
        keep = set(only)
        workloads = [w for w in workloads if w.stem in keep]
    if not workloads:
        raise SystemExit("no workloads matched")

    conn = _connect(db_path)

    import platform
    if sys.platform == "darwin":
        platform_tag = "macos-arm64" if platform.machine() in {"arm64", "aarch64"} else "macos-x64"
    elif sys.platform == "win32":
        platform_tag = "windows-x64"
    else:
        platform_tag = "cpu"

    written = 0
    for wp in workloads:
        workload_id = wp.stem
        task = _task_type(workload_id)
        template = templates[task]
        input_text = _extract_input(wp.read_text(encoding="utf-8"))
        prompt = template.replace("{input}", input_text, 1)
        input_hash = _sha256(input_text)

        print(f"workload {workload_id}: warmup {warmup} + measured {runs}")
        for seq in range(warmup + runs):
            is_warmup = seq < warmup
            sampler = _RAMSampler()
            sampler.start()
            try:
                r = _ollama_generate(ollama_model, prompt, max_new_tokens)
            except Exception as e:
                sampler.stop()
                print(f"  run={seq - warmup if not is_warmup else 'W'} FAIL: {type(e).__name__}: {e}")
                continue
            peak_ram = sampler.stop()
            if is_warmup:
                continue

            tps = (
                r["eval_count"] / (r["eval_duration_ns"] / 1e9)
                if r["eval_duration_ns"] > 0
                else 0.0
            )
            result = BenchResult(
                bench_session_id=session_id,
                model_alias=model_alias,
                model_variant=ollama_model,
                model_repo="ollama",
                model_revision="local",
                workload_id=workload_id,
                ttft_ms=r["ttft_ms"],
                tokens_per_sec=tps,
                peak_ram_mb=peak_ram,
                prompt_tokens=r["prompt_eval_count"],
                completion_tokens=r["eval_count"],
                completion_text=r["response"],
                input_hash=input_hash,
                output_hash=_sha256(r["response"]),
                ep_requested="ollama",
                ep_actual="ollama-metal" if sys.platform == "darwin" else "ollama-cpu",
                ep_fallback=0,
                ep_fallback_reason="",
                run_seq=seq - warmup,
                platform_tag=platform_tag,
                prompt_version=prompt_version,
            )
            cols = list(asdict(result).keys())
            placeholders = ",".join(["?"] * len(cols))
            conn.execute(
                f"INSERT INTO bench_runs ({','.join(cols)}) VALUES ({placeholders})",
                tuple(asdict(result).values()),
            )
            conn.commit()
            written += 1
            print(
                f"  run={seq - warmup} ttft={result.ttft_ms:.0f}ms "
                f"tok/s={tps:.1f} ram={peak_ram:.0f}MB tokens={r['eval_count']}"
            )
    conn.close()
    return written


def _cli() -> None:
    parser = argparse.ArgumentParser(description="Phase-0 Ollama LLM bench (ADR-002)")
    parser.add_argument("--model-alias", required=True, help="logical alias for DB rows (e.g. gemma4-e4b)")
    parser.add_argument("--ollama-model", required=True, help="ollama model tag (e.g. gemma4:e4b)")
    parser.add_argument("--workloads-dir", type=Path, default=INPUTS_DIR)
    parser.add_argument("--prompts", type=Path, default=PROMPTS_PATH)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--max-new-tokens", type=int, default=512)
    parser.add_argument("--db", type=Path, default=DB_PATH)
    parser.add_argument("--only", help="comma-separated workload-ids to keep")
    args = parser.parse_args()
    only = [s.strip() for s in args.only.split(",")] if args.only else None
    n = run_bench(
        model_alias=args.model_alias,
        ollama_model=args.ollama_model,
        workloads_dir=args.workloads_dir,
        prompts_path=args.prompts,
        runs=args.runs,
        warmup=args.warmup,
        max_new_tokens=args.max_new_tokens,
        db_path=args.db,
        only=only,
    )
    print(f"wrote {n} measured rows")


if __name__ == "__main__":
    _cli()
