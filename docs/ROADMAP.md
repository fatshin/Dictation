# Roadmap

## Phase 0 — Technical PoC (1 week)

**Goal**: resolve enough unknowns to commit to Phase 1.

Deliverables:

- Benchmark 4 candidate LLMs (Gemma 4 E4B / E2B, Phi-4-mini, SmolLM3-3B) on macOS + Windows
- Benchmark ASR: WhisperKit (Mac) vs sherpa-onnx (Win) vs whisper.cpp
- A/B: two-stage ASR→LLM vs one-stage audio-native LLM (if Gemma 4 audio input ONNX is available)
- Go/No-Go decision

Hard lines:

- TTFT (first-token) < 2500 ms, target < 1500 ms
- LLM-as-judge quality ≥ 7.0 / 10
- Japanese WER < 15 % on mixed JP/EN recordings
- Peak RAM < 8 GB

Full plan: [PHASE0_POC.md](PHASE0_POC.md).

## Phase 1 — MVP (2–3 weeks)

Runs end-to-end on the developer's machine.

- Tauri 2 shell with three-window layout
- ASR integration (macOS sidecar + Windows sidecar)
- LLM rewrite pipeline with 3 templates: rewrite / summarize / translate
- SQLCipher-backed history
- Global hotkey + cursor-position text injection
- Network-guard verification (`nettop` shows zero egress)
- Consent UI before first recording
- macOS and Windows local builds (unsigned)

No distribution yet. No public release.

## Phase 2 — Language and context (2 weeks)

- Mixed-language (JP/EN) language auto-detection per utterance
- Custom vocabulary (proper-noun injection into ASR hints)
- Per-app tone switching (detect foreground app, apply rewrite style)
- Better IME interop handling

## Phase 3 — Meeting and long-form (1–2 weeks)

- Import meeting audio files (m4a, mp3, wav)
- Long-context summarization (leverage SmolLM3-3B's 128K context)
- Full-text history search with SQLite FTS5
- Export to Markdown / clipboard only (no network export)

## Phase 4 — Signed distribution (2–3 weeks)

- macOS: Developer ID + Notarization → `.dmg`
- Windows: code signing → `.msix`
- Auto-update channel (Sparkle / Tauri updater, signature-verified)
- Public release

## Non-goals

- Cloud sync
- Account system
- Telemetry (not even opt-in until demonstrated need)
- iOS / Android / Linux (possible post-1.0)
- Voice cloning / TTS (different app)
- Pronunciation scoring / language learning (different app)

## Success criteria

- Ships a signed binary that runs offline on both macOS and Windows
- Verifiable by any user with a packet inspector that no audio or text leaves the device
- Matches or exceeds Superwhisper/MacWhisper latency on Apple Silicon
- First-class Japanese mixed-language support that competitors lack

## Release cadence

Once v1.0 ships, point releases every 4–8 weeks. Model updates tracked separately from app releases; users choose which model version they run.
