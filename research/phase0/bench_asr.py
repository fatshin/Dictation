"""ASR WER/CER bench. Platform-picks an engine; falls back to whisper.cpp."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path

from runtime_selector import detect_platform

REPO_ROOT = Path(__file__).resolve().parent
RECORDINGS_DIR = REPO_ROOT / "recordings"
RESULTS_DIR = REPO_ROOT / "results"
REPORT_PATH = RESULTS_DIR / "asr_report.json"


@dataclass
class ASRResult:
    utterance_id: str
    reference: str
    hypothesis: str
    wer: float
    cer: float
    engine: str
    duration_sec: float | None = None
    language: str | None = None


@dataclass
class ASRReport:
    platform: str
    engine: str
    count: int
    wer_avg: float
    cer_avg: float
    items: list[ASRResult] = field(default_factory=list)


def _which(binary: str) -> str | None:
    return shutil.which(binary)


def _select_engine(platform_tag: str) -> str:
    if platform_tag.startswith("macos"):
        if _which("whisperkit-cli"):
            return "whisperkit"
        if _which("whisper-cli") or _which("main"):
            return "whisper.cpp"
        return "whisper.cpp"  # will error at run time if missing; fail-fast
    if platform_tag.startswith("windows"):
        if _which("sherpa-onnx-offline"):
            return "sherpa-onnx"
        return "whisper.cpp"
    return "whisper.cpp"


def _run_whisperkit(wav: Path) -> str:
    cmd = ["whisperkit-cli", "transcribe", "--audio-path", str(wav), "--verbose", "false"]
    out = subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
    return out.strip()


def _run_whisper_cpp(wav: Path, lang: str | None) -> str:
    binary = _which("whisper-cli") or _which("main")
    if not binary:
        raise SystemExit("whisper.cpp binary (whisper-cli or main) not on PATH")
    # Write hypothesis to results/asr_hyp/<stem>.txt — NEVER next to the wav,
    # which would overwrite the reference <stem>.txt and corrupt future runs.
    out_base = RESULTS_DIR / "asr_hyp" / wav.stem
    out_base.parent.mkdir(parents=True, exist_ok=True)
    cmd = [binary, "-f", str(wav), "-otxt", "-of", str(out_base)]
    if lang:
        cmd += ["-l", lang]
    subprocess.run(cmd, check=True, capture_output=True, text=True)
    txt = out_base.with_suffix(".txt")
    return txt.read_text(encoding="utf-8").strip() if txt.exists() else ""


def _run_sherpa(wav: Path) -> str:
    cmd = ["sherpa-onnx-offline", str(wav)]
    out = subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
    return out.strip()


def transcribe(wav: Path, engine: str, language: str | None = None) -> str:
    if engine == "whisperkit":
        return _run_whisperkit(wav)
    if engine == "whisper.cpp":
        return _run_whisper_cpp(wav, language)
    if engine == "sherpa-onnx":
        return _run_sherpa(wav)
    raise SystemExit(f"unknown engine: {engine}")


def _score(reference: str, hypothesis: str) -> tuple[float, float]:
    import jiwer

    wer = float(jiwer.wer(reference, hypothesis))
    cer = float(jiwer.cer(reference, hypothesis))
    return wer, cer


def _iter_pairs(directory: Path) -> list[tuple[Path, Path]]:
    pairs: list[tuple[Path, Path]] = []
    for wav in sorted(directory.glob("*.wav")):
        ref = wav.with_suffix(".txt")
        if not ref.exists():
            print(f"skip (no reference): {wav.name}", file=sys.stderr)
            continue
        pairs.append((wav, ref))
    return pairs


def _infer_lang(ref_text: str) -> str | None:
    has_ja = any("\u3040" <= c <= "\u30ff" or "\u4e00" <= c <= "\u9fff" for c in ref_text)
    has_en = any(c.isascii() and c.isalpha() for c in ref_text)
    if has_ja and not has_en:
        return "ja"
    if has_en and not has_ja:
        return "en"
    return None


def run(directory: Path, engine: str | None = None) -> ASRReport:
    platform_tag = detect_platform()
    chosen = engine or _select_engine(platform_tag)

    items: list[ASRResult] = []
    for wav, ref_file in _iter_pairs(directory):
        reference = ref_file.read_text(encoding="utf-8").strip()
        language = _infer_lang(reference)
        hypothesis = transcribe(wav, chosen, language=language)
        wer, cer = _score(reference, hypothesis)
        items.append(
            ASRResult(
                utterance_id=wav.stem,
                reference=reference,
                hypothesis=hypothesis,
                wer=wer,
                cer=cer,
                engine=chosen,
                language=language,
            )
        )

    if not items:
        return ASRReport(platform=platform_tag, engine=chosen, count=0, wer_avg=0.0, cer_avg=0.0)

    wer_avg = sum(i.wer for i in items) / len(items)
    cer_avg = sum(i.cer for i in items) / len(items)
    return ASRReport(
        platform=platform_tag,
        engine=chosen,
        count=len(items),
        wer_avg=wer_avg,
        cer_avg=cer_avg,
        items=items,
    )


def _serialize(report: ASRReport) -> dict:
    d = asdict(report)
    return d


def _cli() -> None:
    parser = argparse.ArgumentParser(description="ASR WER/CER bench.")
    parser.add_argument("--dir", type=Path, default=RECORDINGS_DIR)
    parser.add_argument("--engine", choices=["whisperkit", "whisper.cpp", "sherpa-onnx"])
    parser.add_argument("--out", type=Path, default=REPORT_PATH)
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    report = run(args.dir, engine=args.engine)
    args.out.write_text(json.dumps(_serialize(report), indent=2, ensure_ascii=False))
    print(json.dumps({"platform": report.platform, "engine": report.engine,
                      "count": report.count, "wer_avg": report.wer_avg,
                      "cer_avg": report.cer_avg}, indent=2))


if __name__ == "__main__":
    _cli()
