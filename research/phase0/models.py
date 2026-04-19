"""Model download, SHA-256 verification, and a 32-token smoke decode."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path

TIER_1: dict[str, str] = {
    "gemma-4-e4b": "onnx-community/gemma-4-E4B-it-ONNX",
    "gemma-4-e2b": "onnx-community/gemma-4-E2B-it-ONNX",
    "phi-4-mini": "microsoft/Phi-4-mini-instruct-onnx",
    "qwen3-4b": "onnx-community/Qwen3-4B-Instruct-2507-ONNX",
}

TIER_2: dict[str, str] = {
    "llama-3.2-3b": "onnx-community/Llama-3.2-3B-Instruct-ONNX",
    "smollm3-3b": "HuggingFaceTB/SmolLM3-3B-ONNX",
}

BACKUP: dict[str, str] = {
    # Prior-generation backup. The onnx-community ONNX repo is gated (401 on
    # anonymous API) at the time of writing, and the Google weights repo ships
    # safetensors, not ONNX. If promoted to Tier 1, plan for either gaining
    # access to the gated ONNX repo or converting the Google weights ourselves.
    "gemma-3n-e4b": "google/gemma-3n-E4B-it",
}

ALL_MODELS: dict[str, str] = {**TIER_1, **TIER_2, **BACKUP}

REPO_ROOT = Path(__file__).resolve().parent
MODELS_DIR = REPO_ROOT / "downloads"
MANIFEST_PATH = REPO_ROOT / "results" / "model_manifest.json"


@dataclass
class SmokeMetrics:
    model_id: str
    model_dir: str
    prompt: str
    completion: str
    ttft_ms: float
    tokens_per_sec: float
    peak_ram_mb: float
    completion_tokens: int


def _ensure_dirs() -> None:
    MODELS_DIR.mkdir(parents=True, exist_ok=True)
    MANIFEST_PATH.parent.mkdir(parents=True, exist_ok=True)


def check_existence(repo_id: str) -> dict:
    """Probe a HF repo for file listing and metadata. Fails fast if missing."""
    from huggingface_hub import HfApi
    from huggingface_hub.utils import RepositoryNotFoundError

    api = HfApi()
    try:
        info = api.repo_info(repo_id=repo_id, repo_type="model", files_metadata=True)
    except RepositoryNotFoundError as e:
        raise SystemExit(f"repo not found: {repo_id}") from e

    files = []
    for sibling in getattr(info, "siblings", []) or []:
        files.append(
            {
                "path": sibling.rfilename,
                "size": getattr(sibling, "size", None),
                "sha": getattr(sibling, "lfs", None) and sibling.lfs.get("sha256"),
            }
        )

    has_genai_config = any(f["path"].endswith("genai_config.json") for f in files)
    return {
        "repo_id": repo_id,
        "revision": getattr(info, "sha", None),
        "last_modified": str(getattr(info, "last_modified", "")),
        "file_count": len(files),
        "files": files,
        "has_genai_config": has_genai_config,
    }


def download(repo_id: str, local_dir: Path, revision: str | None = None) -> Path:
    """Snapshot-download the repo into local_dir. Returns the resolved path."""
    from huggingface_hub import snapshot_download

    local_dir.mkdir(parents=True, exist_ok=True)
    # `local_dir_use_symlinks=False` was deprecated in huggingface_hub 0.23
    # and removed in later releases; specifying `local_dir` alone now copies
    # files directly without symlinks.
    path = snapshot_download(
        repo_id=repo_id,
        local_dir=str(local_dir),
        revision=revision,
    )
    return Path(path)


def _sha256_file(path: Path, buf_size: int = 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        while chunk := fh.read(buf_size):
            digest.update(chunk)
    return digest.hexdigest()


def build_manifest(model_dir: Path) -> dict[str, str]:
    """Walk model_dir and return {relative_path: sha256}."""
    entries: dict[str, str] = {}
    for p in sorted(model_dir.rglob("*")):
        if not p.is_file():
            continue
        if any(part.startswith(".") for part in p.relative_to(model_dir).parts):
            continue
        entries[str(p.relative_to(model_dir))] = _sha256_file(p)
    return entries


def verify_sha256(model_dir: Path, manifest: dict[str, str]) -> bool:
    """Return True iff every file in manifest hashes to the recorded value."""
    for rel, expected in manifest.items():
        fp = model_dir / rel
        if not fp.exists():
            print(f"missing: {rel}", file=sys.stderr)
            return False
        actual = _sha256_file(fp)
        if actual != expected:
            print(f"sha mismatch: {rel} expected={expected} actual={actual}", file=sys.stderr)
            return False
    return True


def _peak_rss_mb() -> float:
    import psutil

    return psutil.Process(os.getpid()).memory_info().rss / (1024 * 1024)


def smoke_decode(model_dir: Path, n_tokens: int = 32) -> dict:
    """Generate a short completion and record TTFT, throughput, peak RAM."""
    import onnxruntime_genai as og

    prompt = "Reply with a single short English sentence."
    model = og.Model(str(model_dir))
    tokenizer = og.Tokenizer(model)

    input_ids = tokenizer.encode(prompt)

    params = og.GeneratorParams(model)
    params.set_search_options(max_length=len(input_ids) + n_tokens, temperature=0.0)

    generator = og.Generator(model, params)
    generator.append_tokens(input_ids)

    peak_ram = _peak_rss_mb()
    t0 = time.perf_counter()
    ttft_ms: float | None = None
    produced: list[int] = []

    while not generator.is_done() and len(produced) < n_tokens:
        generator.generate_next_token()
        if ttft_ms is None:
            ttft_ms = (time.perf_counter() - t0) * 1000.0
        produced.append(int(generator.get_next_tokens()[0]))
        peak_ram = max(peak_ram, _peak_rss_mb())

    elapsed = time.perf_counter() - t0
    completion = tokenizer.decode(produced)
    tokens_per_sec = len(produced) / elapsed if elapsed > 0 else 0.0

    metrics = SmokeMetrics(
        model_id=model_dir.name,
        model_dir=str(model_dir),
        prompt=prompt,
        completion=completion,
        ttft_ms=ttft_ms or 0.0,
        tokens_per_sec=tokens_per_sec,
        peak_ram_mb=peak_ram,
        completion_tokens=len(produced),
    )
    return asdict(metrics)


def _resolve_model(alias_or_dir: str) -> tuple[str, Path]:
    if alias_or_dir in ALL_MODELS:
        repo = ALL_MODELS[alias_or_dir]
        return repo, MODELS_DIR / alias_or_dir
    p = Path(alias_or_dir)
    if p.is_dir():
        return p.name, p
    raise SystemExit(f"unknown model alias or directory: {alias_or_dir}")


def _cmd_check_all(_: argparse.Namespace) -> None:
    out: dict[str, dict] = {}
    for alias, repo in ALL_MODELS.items():
        print(f"checking {alias} -> {repo}")
        out[alias] = check_existence(repo)
    (REPO_ROOT / "results").mkdir(parents=True, exist_ok=True)
    target = REPO_ROOT / "results" / "hf_existence.json"
    target.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(f"wrote {target}")


def _cmd_download(args: argparse.Namespace) -> None:
    _ensure_dirs()
    tier_map = {"1": TIER_1, "2": TIER_2, "backup": BACKUP, "all": ALL_MODELS}
    targets = tier_map.get(args.tier)
    if targets is None:
        raise SystemExit(f"unknown tier: {args.tier}")

    manifest: dict[str, dict[str, str]] = {}
    if MANIFEST_PATH.exists():
        manifest = json.loads(MANIFEST_PATH.read_text())

    for alias, repo in targets.items():
        dest = MODELS_DIR / alias
        print(f"downloading {alias} ({repo}) -> {dest}")
        resolved = download(repo, dest)
        manifest[alias] = build_manifest(resolved)
        MANIFEST_PATH.write_text(json.dumps(manifest, indent=2))
        print(f"wrote manifest entry for {alias} ({len(manifest[alias])} files)")


def _cmd_smoke(args: argparse.Namespace) -> None:
    _, model_dir = _resolve_model(args.model)
    if not model_dir.exists():
        raise SystemExit(f"model dir missing, run download first: {model_dir}")
    metrics = smoke_decode(model_dir, n_tokens=args.tokens)
    print(json.dumps(metrics, indent=2, ensure_ascii=False))


def _cmd_verify(args: argparse.Namespace) -> None:
    if not MANIFEST_PATH.exists():
        raise SystemExit("no manifest yet; run `download` first")
    manifest = json.loads(MANIFEST_PATH.read_text())
    for alias in args.models or list(manifest.keys()):
        entry = manifest.get(alias)
        if not entry:
            print(f"{alias}: NO MANIFEST")
            continue
        ok = verify_sha256(MODELS_DIR / alias, entry)
        print(f"{alias}: {'OK' if ok else 'FAIL'}")


def _cli() -> None:
    parser = argparse.ArgumentParser(description="Phase 0 model download and smoke tool.")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("check-all").set_defaults(func=_cmd_check_all)

    p_dl = sub.add_parser("download")
    p_dl.add_argument("tier", choices=["1", "2", "backup", "all"])
    p_dl.set_defaults(func=_cmd_download)

    p_sm = sub.add_parser("smoke")
    p_sm.add_argument("model", help="alias from TIER_1/TIER_2/BACKUP or path to model dir")
    p_sm.add_argument("--tokens", type=int, default=32)
    p_sm.set_defaults(func=_cmd_smoke)

    p_ver = sub.add_parser("verify")
    p_ver.add_argument("models", nargs="*")
    p_ver.set_defaults(func=_cmd_verify)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    _cli()
