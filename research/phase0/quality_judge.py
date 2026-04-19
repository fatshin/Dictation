"""LLM-as-judge using Claude Opus 4.7, with an on-disk cache."""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import re
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent
RESULTS_DIR = REPO_ROOT / "results"
CACHE_PATH = RESULTS_DIR / "judge_cache.sqlite"
BENCH_DB_PATH = RESULTS_DIR / "bench_db.sqlite"

JUDGE_MODEL = "claude-opus-4-7"

TASK_PROMPTS: dict[str, str] = {
    "ja_keigo": (
        "Japanese business register. Judge the rewrite for polite-form accuracy, "
        "filler removal, semantic preservation, and structural clarity."
    ),
    "jp_en_mix": (
        "Japanese/English code-switching. Judge politeness, filler removal, "
        "meaning preservation, and verbatim retention of technical terms."
    ),
    "en_business": (
        "English business register. Judge tone, filler removal, meaning preservation, "
        "and sentence completion."
    ),
    "summary": (
        "Long-form summarization. Judge faithfulness, filler/noise removal, "
        "coverage of key points, and structural clarity (3-line + action items)."
    ),
}

SCHEMA = """
CREATE TABLE IF NOT EXISTS judge_scores (
    model_id    TEXT NOT NULL,
    input_hash  TEXT NOT NULL,
    output_hash TEXT NOT NULL,
    task_type   TEXT NOT NULL,
    keigo       REAL NOT NULL,
    filler      REAL NOT NULL,
    semantic    REAL NOT NULL,
    structure   REAL NOT NULL,
    raw_json    TEXT NOT NULL,
    judge_model TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    PRIMARY KEY (model_id, input_hash, output_hash)
);
"""

SYSTEM_PROMPT = (
    "You grade dictation-rewrite outputs on four axes, each 0-10. "
    "Return JSON only, no prose. Keys: keigo, filler, semantic, structure. "
    "Values are numbers (integers or one decimal). Be strict; 10 is rare."
)


@dataclass
class JudgeScores:
    keigo: float
    filler: float
    semantic: float
    structure: float
    raw_json: str


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _connect(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(path)
    conn.executescript(SCHEMA)
    return conn


def _cache_get(
    conn: sqlite3.Connection, model_id: str, input_hash: str, output_hash: str
) -> dict | None:
    cur = conn.execute(
        "SELECT keigo, filler, semantic, structure, raw_json FROM judge_scores "
        "WHERE model_id=? AND input_hash=? AND output_hash=?",
        (model_id, input_hash, output_hash),
    )
    row = cur.fetchone()
    if not row:
        return None
    return {"keigo": row[0], "filler": row[1], "semantic": row[2], "structure": row[3], "raw_json": row[4]}


def _cache_put(
    conn: sqlite3.Connection,
    model_id: str,
    input_hash: str,
    output_hash: str,
    task_type: str,
    scores: JudgeScores,
) -> None:
    conn.execute(
        """INSERT OR REPLACE INTO judge_scores
           (model_id, input_hash, output_hash, task_type, keigo, filler, semantic, structure,
            raw_json, judge_model, timestamp)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
        (
            model_id,
            input_hash,
            output_hash,
            task_type,
            scores.keigo,
            scores.filler,
            scores.semantic,
            scores.structure,
            scores.raw_json,
            JUDGE_MODEL,
            _dt.datetime.utcnow().isoformat(timespec="seconds"),
        ),
    )
    conn.commit()


def _extract_json(text: str) -> dict:
    # strip code fences if the model wrapped output
    cleaned = text.strip()
    cleaned = re.sub(r"^```(?:json)?\s*|\s*```$", "", cleaned, flags=re.MULTILINE)
    match = re.search(r"\{.*\}", cleaned, re.DOTALL)
    payload = match.group(0) if match else cleaned
    return json.loads(payload)


def judge(input_text: str, output_text: str, task_type: str) -> dict:
    """Call the Claude Opus 4.7 judge and return per-axis scores."""
    import anthropic

    if task_type not in TASK_PROMPTS:
        raise SystemExit(f"unknown task_type: {task_type}")

    client = anthropic.Anthropic()
    user = (
        f"Task: {TASK_PROMPTS[task_type]}\n\n"
        f"INPUT (raw dictation):\n{input_text}\n\n"
        f"OUTPUT (rewrite):\n{output_text}\n\n"
        "Return JSON: {\"keigo\": <0-10>, \"filler\": <0-10>, "
        "\"semantic\": <0-10>, \"structure\": <0-10>}"
    )

    message = client.messages.create(
        model=JUDGE_MODEL,
        max_tokens=256,
        temperature=0.0,
        system=SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user}],
    )
    text_parts = [b.text for b in message.content if getattr(b, "type", None) == "text"]
    raw = "\n".join(text_parts).strip()
    parsed = _extract_json(raw)
    return {
        "keigo": float(parsed["keigo"]),
        "filler": float(parsed["filler"]),
        "semantic": float(parsed["semantic"]),
        "structure": float(parsed["structure"]),
        "raw_json": raw,
    }


def _infer_task_type(workload_id: str) -> str:
    if workload_id.startswith("ja_keigo"):
        return "ja_keigo"
    if workload_id.startswith("jp_en_mix"):
        return "jp_en_mix"
    if workload_id.startswith("en_business"):
        return "en_business"
    if workload_id.startswith("summary"):
        return "summary"
    raise SystemExit(f"cannot infer task_type from workload_id={workload_id}")


def _load_input_text(workload_id: str) -> str:
    candidate = REPO_ROOT / "inputs" / f"{workload_id}.txt"
    if not candidate.exists():
        raise SystemExit(f"input file missing: {candidate}")
    return candidate.read_text(encoding="utf-8").strip()


def judge_from_db(bench_db: Path, cache_db: Path, limit: int | None = None) -> list[dict]:
    """Iterate bench_runs, call judge for unseen (model, input, output), return records."""
    conn_bench = sqlite3.connect(bench_db)
    conn_cache = _connect(cache_db)
    records: list[dict] = []
    try:
        q = (
            "SELECT DISTINCT model_id, workload_id, input_hash, output_hash, completion_text "
            "FROM bench_runs"
        )
        if limit is not None:
            q += f" LIMIT {int(limit)}"
        for row in conn_bench.execute(q):
            model_id, workload_id, input_hash, output_hash, completion = row
            cached = _cache_get(conn_cache, model_id, input_hash, output_hash)
            if cached:
                records.append({"model_id": model_id, "workload_id": workload_id, **cached})
                continue

            task_type = _infer_task_type(workload_id)
            input_text = _load_input_text(workload_id)
            print(f"judging: {model_id} x {workload_id}", file=sys.stderr)
            scored = judge(input_text, completion, task_type)
            scores = JudgeScores(**scored)
            _cache_put(conn_cache, model_id, input_hash, output_hash, task_type, scores)
            records.append(
                {
                    "model_id": model_id,
                    "workload_id": workload_id,
                    "keigo": scores.keigo,
                    "filler": scores.filler,
                    "semantic": scores.semantic,
                    "structure": scores.structure,
                    "raw_json": scores.raw_json,
                }
            )
    finally:
        conn_bench.close()
        conn_cache.close()
    return records


def _cli() -> None:
    parser = argparse.ArgumentParser(description="LLM-as-judge with cache.")
    parser.add_argument("--db", type=Path, default=BENCH_DB_PATH)
    parser.add_argument("--cache", type=Path, default=CACHE_PATH)
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--limit", type=int, default=None)
    args = parser.parse_args()

    if not args.db.exists():
        raise SystemExit(f"bench db missing: {args.db}")
    if not (args.all or args.limit):
        raise SystemExit("use --all or --limit N")

    records = judge_from_db(args.db, args.cache, limit=args.limit)
    print(json.dumps(records, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    _cli()
