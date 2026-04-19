"""Latency + quality benchmarking for ONNX GenAI models.

Day 2.5 revision: schema is append-only, model identity is split across
alias/variant/repo/revision, EP provenance records both requested and actual
providers, prompt template is language-aware. See docs/PHASE0_POC.md §Day 2.5.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import sqlite3
import sys
import threading
import time
import uuid
from dataclasses import asdict, dataclass, field
from pathlib import Path

from runtime_selector import detect_platform, select_execution_providers

REPO_ROOT = Path(__file__).resolve().parent
INPUTS_DIR = REPO_ROOT / "inputs"
RESULTS_DIR = REPO_ROOT / "results"
MODELS_DIR = REPO_ROOT / "downloads"
DB_PATH = RESULTS_DIR / "bench_db.sqlite"

PROMPT_VERSION = "2026-04-v2"

# Language-aware prompt templates. Day-2 outputs showed Phi emitting English
# for Japanese input under the old single-template prompt: scoring those would
# grade prompt design, not model quality.
PROMPT_TEMPLATES: dict[str, str] = {
    "ja_keigo": (
        "あなたは音声口述を清書するアシスタントです。"
        "入力は日本語の話し言葉。以下の規則で書き直してください:\n"
        "- **必ず日本語で出力**（英訳・要約禁止）\n"
        "- 敬体（です・ます調）の書き言葉に統一\n"
        "- フィラー（えー、あの、まあ 等）を削除\n"
        "- 意味を保ち、固有名詞・技術用語は原文の表記を維持\n\n"
        "入力:\n{input}\n\n清書:\n"
    ),
    "jp_en_mix": (
        "あなたは音声口述を清書するアシスタントです。"
        "入力は日本語と英語が混在したエンジニア/PMの話し言葉。以下の規則で書き直してください:\n"
        "- **入力の主言語（日本語）を維持**（英訳禁止）\n"
        "- 日本語は敬体の書き言葉、英語の技術用語は**原文のまま**保持\n"
        "- フィラーを削除し、文として完結させる\n"
        "- 意味は忠実に保つ\n\n"
        "入力:\n{input}\n\n清書:\n"
    ),
    "en_business": (
        "You rewrite spoken dictation into polished business English.\n"
        "- Output **English only** (do not translate to other languages).\n"
        "- Use a formal-email register; remove fillers (um, uh, like, you know).\n"
        "- Preserve meaning and any technical terms verbatim.\n"
        "- Complete sentence fragments.\n\n"
        "INPUT:\n{input}\n\nREWRITE:\n"
    ),
    "summary": (
        "You summarize long-form dictation (1-on-1 transcripts, meetings).\n"
        "- Preserve the **original language** of the input.\n"
        "- Output format: 3-line summary, then `Action items:` as a bullet list.\n"
        "- Drop fillers and digressions; keep decisions and commitments.\n\n"
        "INPUT:\n{input}\n\nSUMMARY:\n"
    ),
}


def _task_type_for(workload_id: str) -> str:
    for prefix in ("ja_keigo", "jp_en_mix", "en_business", "summary"):
        if workload_id.startswith(prefix):
            return prefix
    raise SystemExit(f"cannot derive task_type from workload_id={workload_id}")


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
    prompt_version: str = PROMPT_VERSION
    timestamp: str = field(default_factory=lambda: _dt.datetime.now(_dt.UTC).isoformat(timespec="seconds"))


# Append-only schema. No UNIQUE constraint: each bench session is a new
# audit-trail row, old runs are never silently overwritten.
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


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _peak_rss_mb() -> float:
    """Total RSS of this process + children."""
    import psutil

    proc = psutil.Process(os.getpid())
    total = proc.memory_info().rss
    for child in proc.children(recursive=True):
        try:
            total += child.memory_info().rss
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return total / (1024 * 1024)


class _RAMSampler:
    """Background RAM sampler: captures prefill peaks that per-token sampling misses."""

    def __init__(self, interval_sec: float = 0.05) -> None:
        self._interval = interval_sec
        self._peak = 0.0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._peak = _peak_rss_mb()
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                self._peak = max(self._peak, _peak_rss_mb())
            except Exception:
                pass
            self._stop.wait(self._interval)

    def stop(self) -> float:
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=1.0)
        # Final sample to catch anything between last tick and stop.
        self._peak = max(self._peak, _peak_rss_mb())
        return self._peak


def _connect(db_path: Path) -> sqlite3.Connection:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.executescript(_SCHEMA)
    return conn


def store_result(db_path: Path, result: BenchResult) -> None:
    conn = _connect(db_path)
    try:
        cols = list(asdict(result).keys())
        placeholders = ",".join(["?"] * len(cols))
        conn.execute(
            f"INSERT INTO bench_runs ({','.join(cols)}) VALUES ({placeholders})",
            tuple(asdict(result).values()),
        )
        conn.commit()
    finally:
        conn.close()


def _extract_section(text: str, name: str) -> str:
    """Pull out a `## {name}` block without leaking EXPECTED / NOTES."""
    marker = f"## {name}"
    try:
        start = text.index(marker) + len(marker)
    except ValueError as e:
        raise SystemExit(f"section '{name}' not found in workload") from e
    rest = text[start:]
    end = rest.find("\n## ")
    return (rest[:end] if end >= 0 else rest).strip()


def _load_workload(path: Path) -> str:
    if not path.exists():
        raise SystemExit(f"workload missing: {path}")
    raw = path.read_text(encoding="utf-8")
    return _extract_section(raw, "INPUT")


def _render_prompt(workload_text: str, workload_id: str) -> str:
    task = _task_type_for(workload_id)
    return PROMPT_TEMPLATES[task].format(input=workload_text)


def _locate_genai_config(model_dir: Path) -> Path:
    """Find the dir containing genai_config.json. Prefers CPU/mobile INT4 variants."""
    if (model_dir / "genai_config.json").exists():
        return model_dir
    # Prefer cpu_and_mobile/* for Phase 0 (CPU + CoreML bench).
    hits: list[Path] = [
        p.parent
        for p in model_dir.rglob("genai_config.json")
        if ".cache" not in p.parts
    ]
    if not hits:
        raise SystemExit(f"no genai_config.json under {model_dir}")

    def _score(p: Path) -> tuple[int, str]:
        parts = {x.lower() for x in p.parts}
        s = 0
        if "cpu_and_mobile" in parts:
            s -= 10
        if any("int4" in x.lower() for x in p.parts):
            s -= 5
        if any("gpu" in x.lower() for x in p.parts):
            s += 3
        return (s, str(p))

    hits.sort(key=_score)
    return hits[0]


def _read_variant(model_dir: Path, genai_dir: Path) -> str:
    rel = genai_dir.relative_to(model_dir)
    return str(rel) if str(rel) != "." else "root"


def _read_model_repo_revision(alias: str, model_dir: Path) -> tuple[str, str]:
    try:
        from models import ALL_MODELS  # type: ignore

        repo = ALL_MODELS.get(alias, "")
    except Exception:
        repo = ""
    manifest_path = RESULTS_DIR / "model_manifest.json"
    revision = ""
    if manifest_path.exists():
        try:
            m = json.loads(manifest_path.read_text())
            entry = m.get(alias)
            if isinstance(entry, dict):
                revision = str(entry.get("revision", ""))
        except Exception:
            revision = ""
    # Fallback revision: top-level dir mtime stamp so audit row is never empty.
    if not revision:
        revision = f"local-mtime-{int(model_dir.stat().st_mtime)}"
    return repo, revision


def run_bench(
    model_alias: str,
    model_dir: Path,
    workload_path: Path,
    runs: int = 5,
    warmup: int = 2,
    max_new_tokens: int = 512,
    bench_session_id: str | None = None,
) -> list[BenchResult]:
    """Run `warmup + runs` generations and return the `runs` measured results."""
    import onnxruntime_genai as og

    session_id = bench_session_id or uuid.uuid4().hex
    platform_tag = detect_platform()

    genai_dir = _locate_genai_config(model_dir)
    variant = _read_variant(model_dir, genai_dir)
    repo, revision = _read_model_repo_revision(model_alias, model_dir)

    requested = select_execution_providers()
    ep_requested_tag = ",".join(requested)

    workload_text = _load_workload(workload_path)
    workload_id = workload_path.stem
    prompt = _render_prompt(workload_text, workload_id)
    input_hash = _sha256(workload_text)

    ep_actual = ep_requested_tag
    ep_fallback = 0
    ep_fallback_reason = ""
    try:
        config = og.Config(str(genai_dir))
        if hasattr(config, "clear_providers"):
            config.clear_providers()
        for ep in requested:
            if hasattr(config, "append_provider"):
                config.append_provider(ep)
        model = og.Model(config)
    except (AttributeError, TypeError, RuntimeError) as exc:
        # Config-based EP selection is unstable across onnxruntime-genai
        # releases. Fall back to genai_config.json-declared EPs; record the
        # divergence so audit queries can filter these rows.
        model = og.Model(str(genai_dir))
        ep_actual = "genai_config_default"
        ep_fallback = 1
        ep_fallback_reason = f"{type(exc).__name__}: {exc}"
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

        sampler = _RAMSampler(interval_sec=0.05)
        generator = og.Generator(model, params)
        produced: list[int] = []
        sampler.start()
        t0 = time.perf_counter()
        generator.append_tokens(input_ids)
        ttft_ms: float | None = None

        while not generator.is_done() and len(produced) < max_new_tokens:
            generator.generate_next_token()
            if ttft_ms is None:
                ttft_ms = (time.perf_counter() - t0) * 1000.0
            produced.append(int(generator.get_next_tokens()[0]))

        elapsed = time.perf_counter() - t0
        peak_ram = sampler.stop()
        if is_warmup:
            continue

        completion = tokenizer.decode(produced)
        results.append(BenchResult(
            bench_session_id=session_id,
            model_alias=model_alias,
            model_variant=variant,
            model_repo=repo,
            model_revision=revision,
            workload_id=workload_id,
            ttft_ms=ttft_ms or 0.0,
            tokens_per_sec=(len(produced) / elapsed) if elapsed > 0 else 0.0,
            peak_ram_mb=peak_ram,
            prompt_tokens=len(input_ids),
            completion_tokens=len(produced),
            completion_text=completion,
            input_hash=input_hash,
            output_hash=_sha256(completion),
            ep_requested=ep_requested_tag,
            ep_actual=ep_actual,
            ep_fallback=ep_fallback,
            ep_fallback_reason=ep_fallback_reason,
            run_seq=seq - warmup,
            platform_tag=platform_tag,
        ))

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


def _resolve_one_model(raw: str, label: str | None) -> tuple[str, Path]:
    """Resolve a model arg into (alias, dir). Alias lookup first; `--label` is escape hatch."""
    from models import ALL_MODELS  # type: ignore

    if raw in ALL_MODELS:
        return raw, MODELS_DIR / raw
    p = Path(raw)
    if not p.is_dir():
        raise SystemExit(f"unknown model: {raw}")
    if label is None:
        raise SystemExit(
            f"bare path '{raw}' requires --label to pin the model_alias "
            "(otherwise DB rows would inherit the subdir basename)."
        )
    return label, p


def _cli() -> None:
    parser = argparse.ArgumentParser(description="Per-model latency + quality bench.")
    parser.add_argument("--model", help="alias (see models.py) or path to model dir")
    parser.add_argument("--label", help="explicit model_alias when --model is a bare path")
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
        alias, path = _resolve_one_model(args.model, args.label)
        model_map = {alias: path}
        workloads = _iter_workloads([args.workload or args.workloads_dir])

    if not workloads:
        raise SystemExit("no workloads found")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    session_id = uuid.uuid4().hex
    print(f"bench_session_id={session_id}")

    for alias, model_dir in model_map.items():
        if not model_dir.exists():
            print(f"skip (not downloaded): {alias}", file=sys.stderr)
            continue
        for wp in workloads:
            print(f"bench: {alias} x {wp.name}")
            results = run_bench(
                model_alias=alias,
                model_dir=model_dir,
                workload_path=wp,
                runs=args.runs,
                warmup=args.warmup,
                max_new_tokens=args.max_new_tokens,
                bench_session_id=session_id,
            )
            for r in results:
                store_result(args.db, r)
                print(
                    f"  run={r.run_seq} ttft={r.ttft_ms:.1f}ms "
                    f"tok/s={r.tokens_per_sec:.1f} ram={r.peak_ram_mb:.0f}MB "
                    f"ep_actual={r.ep_actual} fb={r.ep_fallback}"
                )
            summary = {
                "bench_session_id": session_id,
                "model_alias": alias,
                "workload_id": wp.stem,
                "runs_recorded": len(results),
            }
            print(json.dumps({"summary": summary}))


if __name__ == "__main__":
    _cli()
