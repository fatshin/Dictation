"""
Clean-isolation speed check for Dictation model candidates.

Each model is benched with keep_alive=0 injected between runs so no other
model stays resident during measurement.  Reports TTFT (p50/p95) and tok/s
for the three representative workloads used in Dictation.

Usage:
    cd research/phase0
    uv run speed_check.py          # all candidates
    uv run speed_check.py --only qwen3.5:4b-q4_K_M gemma4:e4b
"""
from __future__ import annotations

import argparse
import json
import statistics
import time
import urllib.request
from pathlib import Path

OLLAMA_HOST = "http://127.0.0.1:11434"
REPO_ROOT = Path(__file__).resolve().parent

CANDIDATES = [
    ("qwen3.5:4b-q4_K_M",                              "qwen3.5-4b"),
    ("qwen3.5:9b-q4_K_M",                              "qwen3.5-9b"),
    ("gemma4:e4b",                                      "gemma4-e4b"),
    ("gemma4:e2b",                                      "gemma4-e2b"),
    ("hf.co/alfredplpl/llm-jp-3-3.7b-instruct-gguf:Q4_K_M", "llm-jp-3-3.7b"),
]

# Three representative Dictation workloads
WORKLOADS = ["ja_keigo_01", "en_business_01", "jp_en_mix_01"]

PROMPTS_PATH = REPO_ROOT / "prompt_templates" / "2026-04-v2.json"
INPUTS_DIR   = REPO_ROOT / "inputs"

WARMUP = 1
RUNS   = 3
MAX_NEW = 256  # enough for a full rewrite, not inflated


def _post(payload: dict, timeout: int = 300) -> dict:
    data = json.dumps(payload).encode()
    req  = urllib.request.Request(
        f"{OLLAMA_HOST}/api/generate",
        data=data,
        headers={"Content-Type": "application/json"},
    )
    chunks: list[str] = []
    t0 = time.perf_counter()
    ttft_ms: float | None = None
    last: dict = {}
    with urllib.request.urlopen(req, timeout=timeout) as r:
        for line in r:
            d = json.loads(line)
            tok = d.get("response", "")
            if tok and ttft_ms is None:
                ttft_ms = (time.perf_counter() - t0) * 1000.0
            chunks.append(tok)
            if d.get("done"):
                last = d
                break
    return {
        "ttft_ms":   ttft_ms or 0.0,
        "eval_count": last.get("eval_count", 0),
        "eval_duration_ns": last.get("eval_duration", 0),
    }


def evict(model: str) -> None:
    """Force-unload a model from Ollama keep_alive."""
    payload = {
        "model": model,
        "prompt": "",
        "keep_alive": 0,
        "stream": False,
    }
    data = json.dumps(payload).encode()
    req  = urllib.request.Request(
        f"{OLLAMA_HOST}/api/generate",
        data=data,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30):
            pass
    except Exception:
        pass  # model may already be unloaded


def ollama_resident() -> list[str]:
    try:
        req = urllib.request.Request(f"{OLLAMA_HOST}/api/ps")
        with urllib.request.urlopen(req, timeout=5) as r:
            data = json.loads(r.read())
        return [m["name"] for m in data.get("models", [])]
    except Exception:
        return []


def _extract_input(text: str) -> str:
    marker = "## INPUT"
    start  = text.index(marker) + len(marker)
    rest   = text[start:]
    end    = rest.find("\n## ")
    return (rest[:end] if end >= 0 else rest).strip()


def _task_type(workload_id: str) -> str:
    for prefix in ("ja_keigo", "jp_en_mix", "en_business", "summary"):
        if workload_id.startswith(prefix):
            return prefix
    raise ValueError(f"unknown workload: {workload_id}")


def bench_model(tag: str, alias: str, prompts: dict, workload_ids: list[str]) -> dict:
    templates = prompts["templates"]
    rows: list[dict] = []

    for wid in workload_ids:
        wp      = INPUTS_DIR / f"{wid}.txt"
        task    = _task_type(wid)
        template = templates[task]
        input_text = _extract_input(wp.read_text(encoding="utf-8"))
        prompt  = template.replace("{input}", input_text, 1)

        payload = {
            "model":   tag,
            "prompt":  prompt,
            "stream":  True,
            "think":   False,
            "options": {"temperature": 0.0, "num_predict": MAX_NEW},
        }

        print(f"    {wid}: ", end="", flush=True)
        for seq in range(WARMUP + RUNS):
            is_warmup = seq < WARMUP
            try:
                r = _post(payload)
            except Exception as e:
                print(f"FAIL({e}) ", end="", flush=True)
                continue
            tps = (
                r["eval_count"] / (r["eval_duration_ns"] / 1e9)
                if r["eval_duration_ns"] > 0 else 0.0
            )
            label = "W" if is_warmup else str(seq - WARMUP)
            print(f"[{label}:{r['ttft_ms']:.0f}ms/{tps:.0f}tok/s] ", end="", flush=True)
            if not is_warmup:
                rows.append({"workload": wid, "ttft_ms": r["ttft_ms"], "tps": tps})
        print()

    return rows


def summarise(rows: list[dict]) -> dict:
    ttfts = [r["ttft_ms"] for r in rows]
    tpss  = [r["tps"]     for r in rows]
    if not ttfts:
        return {}
    ttfts_sorted = sorted(ttfts)
    return {
        "ttft_p50": statistics.median(ttfts),
        "ttft_p95": ttfts_sorted[int(len(ttfts_sorted) * 0.95)],
        "ttft_min": min(ttfts),
        "tps_median": statistics.median(tpss),
        "tps_max":    max(tpss),
        "n": len(ttfts),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", nargs="+", help="ollama tags to bench (subset)")
    args = parser.parse_args()

    prompts = json.loads(PROMPTS_PATH.read_text(encoding="utf-8"))

    candidates = CANDIDATES
    if args.only:
        keep = set(args.only)
        candidates = [(t, a) for t, a in candidates if t in keep or a in keep]

    # check which are actually pulled
    pulled: set[str] = set()
    try:
        req = urllib.request.Request(f"{OLLAMA_HOST}/api/tags")
        with urllib.request.urlopen(req, timeout=5) as r:
            tags_data = json.loads(r.read())
        pulled = {m["name"] for m in tags_data.get("models", [])}
    except Exception:
        print("WARNING: cannot reach ollama — is it running?")
        return

    print("=" * 60)
    print(f"Speed check — {len(candidates)} candidates, {WARMUP}W+{RUNS} runs")
    print(f"Workloads: {WORKLOADS}")
    print("=" * 60)

    results: dict[str, dict] = {}

    for tag, alias in candidates:
        if tag not in pulled:
            print(f"\n[SKIP] {alias} ({tag}) — not pulled")
            continue

        # evict everything else before loading this model
        for resident in ollama_resident():
            if resident != tag:
                print(f"  evicting {resident} ...", end=" ", flush=True)
                evict(resident)
                print("done")
        time.sleep(1)

        print(f"\n[bench] {alias} ({tag})")
        rows = bench_model(tag, alias, prompts, WORKLOADS)
        evict(tag)  # clean up after
        time.sleep(1)

        stats = summarise(rows)
        results[alias] = {"tag": tag, **stats}

    # --- report ---
    print("\n" + "=" * 60)
    print("RESULTS (clean-isolation bench)")
    print("=" * 60)
    header = f"{'Model':<22} {'TTFT p50':>10} {'TTFT p95':>10} {'TTFT min':>10} {'tok/s p50':>10} {'tok/s max':>10} {'Verdict'}"
    print(header)
    print("-" * len(header))

    HARD_TTFT   = 2500   # ms
    TARGET_TTFT = 1500   # ms

    for alias, s in results.items():
        if not s:
            print(f"{alias:<22}  NO DATA")
            continue
        verdict = "PASS" if s["ttft_p95"] < HARD_TTFT else "FAIL-TTFT"
        flag    = " ★" if s["ttft_p95"] < TARGET_TTFT else ""
        print(
            f"{alias:<22} {s['ttft_p50']:>9.0f}ms {s['ttft_p95']:>9.0f}ms "
            f"{s['ttft_min']:>9.0f}ms {s['tps_median']:>9.0f}/s "
            f"{s['tps_max']:>9.0f}/s  {verdict}{flag}"
        )

    print()
    print(f"Hard line: TTFT p95 < {HARD_TTFT} ms")
    print(f"Target   : TTFT p95 < {TARGET_TTFT} ms  (marked ★)")

    out = REPO_ROOT / "results" / "speed_check.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(results, indent=2, ensure_ascii=False))
    print(f"\nSaved: {out}")


if __name__ == "__main__":
    main()
