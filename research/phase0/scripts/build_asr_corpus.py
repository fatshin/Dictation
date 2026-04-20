"""Generate a synthetic ASR corpus from workload INPUT sections via macOS `say`.

This is a SMOKE baseline, not a realistic dictation corpus. `say` TTS produces
clean prosody with no fillers, so WER/CER here reflects ASR fidelity on
perfect speech — a floor, not a field measurement. Real dictation recordings
are a Phase-1 task (requires consent UI, privacy review).

Corpus layout: `recordings/<stem>.wav` + `recordings/<stem>.txt` (reference).
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

PHASE0 = Path(__file__).resolve().parent.parent
INPUTS = PHASE0 / "inputs"
REC = PHASE0 / "recordings"

SELECTED = [
    ("ja_keigo_01", "Kyoko"),
    ("ja_keigo_02", "Kyoko"),
    ("jp_en_mix_01", "Kyoko"),
    ("jp_en_mix_02", "Kyoko"),
    ("en_business_01", "Samantha"),
    ("en_business_02", "Samantha"),
]


def _extract_input(raw: str) -> str:
    marker = "## INPUT"
    start = raw.index(marker) + len(marker)
    rest = raw[start:]
    end = rest.find("\n## ")
    return (rest[:end] if end >= 0 else rest).strip()


def synthesize(stem: str, voice: str) -> None:
    src = INPUTS / f"{stem}.txt"
    if not src.exists():
        print(f"skip missing: {src.name}", file=sys.stderr)
        return
    text = _extract_input(src.read_text(encoding="utf-8"))

    aiff = REC / f"{stem}.aiff"
    wav = REC / f"{stem}.wav"
    ref = REC / f"{stem}.txt"

    REC.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["say", "-v", voice, "-o", str(aiff), text],
        check=True,
    )
    # whisper.cpp prefers 16kHz mono WAV.
    subprocess.run(
        [
            "afconvert", str(aiff),
            "-d", "LEI16@16000",
            "-c", "1",
            "-f", "WAVE",
            str(wav),
        ],
        check=True,
    )
    aiff.unlink(missing_ok=True)
    ref.write_text(text, encoding="utf-8")
    print(f"made: {wav.name} ({wav.stat().st_size // 1024} KB)")


def main() -> None:
    for stem, voice in SELECTED:
        synthesize(stem, voice)


if __name__ == "__main__":
    main()
