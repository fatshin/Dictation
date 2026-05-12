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

## Phase 1 — MVP

Runs end-to-end on the developer's machine. Split into two sub-phases to de-risk cross-platform work. Honest estimate for one person: 6–8 weeks.

### Phase 1a — macOS-first MVP (4 weeks)

- Tauri 2 shell with three-window layout
- macOS ASR integration (`whisper-rs` in-process, Metal backend)
- LLM rewrite via `candle-transformers` (pinned 0.9.x), GGUF on Metal
- Rewrite pipeline with 3 templates: rewrite / summarize / translate
- SQLCipher-backed history on macOS keystore
- Global hotkey + cursor-position text injection
- Network-guard verification (`nettop` shows zero egress)
- Consent UI before first recording
- macOS local builds (unsigned)

### Phase 1b — Windows parity (**~7-9 weeks**, honest re-estimate)

Detailed plan: [WINDOWS_PLAN.md](WINDOWS_PLAN.md).

Phases W0-W8:
- W0: Build green on Windows CI (2-3 d)
- W1: Whisper-only path end-to-end (1 wk)
- W2: Global hotkey + right-Ctrl PTT analogue (3-5 d)
- W3: UI Automation for focused-field context (1-2 wk)
- W4: DPAPI keystore (3-5 d)
- W5: Focus tracking + arm-and-paste (3-5 d)
- W6: Ollama Windows x64 / ARM64 integration (1 wk)
- W7: MSI + OV code-signing (1-2 wk, includes cert lead time)
- W8: Cross-platform regression pass (3-5 d)

The original "~2 weeks" estimate assumed the Windows-side modules
already existed; in reality they are stubs. The Windows-ARM NPU
acceleration caveat from ADR-001 remains open and is tracked in
WR-1 of WINDOWS_PLAN.md.

No distribution yet. No public release.

## Phase 2 — Language and context (2 weeks)

- Mixed-language (JP/EN) language auto-detection per utterance
- Custom vocabulary (proper-noun injection into ASR hints)
- Per-app tone switching (detect foreground app, apply rewrite style)
- Better IME interop handling

## Phase 3 — Meeting and long-form (5–7 weeks, honest re-estimate)

Detailed plan: [SYSTEM_AUDIO_CAPTURE_PLAN.md](SYSTEM_AUDIO_CAPTURE_PLAN.md).

Headline feature: **system audio loopback capture** (PLAUD AI-style)
so meeting audio from Zoom / Teams / Meet can be transcribed
alongside the mic, all on-device. Phases S0-S5:

- S0: macOS ScreenCaptureKit + Windows WASAPI loopback spike (3-5 d)
- S1: dual-source capture in the real app (1-2 wk)
- S2: per-stream transcription + time-sorted merge (1 wk)
- S3: UI + consent dialog + recording-state visibility (1 wk)
- S4: meeting-minutes + action-items LLM templates (3-5 d)
- S5: file import (m4a, mp3, wav) (3-5 d)

Other Phase 3 items rolled in:
- Long-context summarization (leverage 128K-context models)
- Full-text history search with SQLite FTS5
- Export to Markdown / clipboard only (no network export)

The dominant risk for this phase is legal (consent for recording
remote parties), not technical. See SYSTEM_AUDIO_CAPTURE_PLAN.md
§ legal+ethical for the disclaimer flow.

## Phase 4 — Signed distribution (3–5 weeks, honest re-estimate)

Detailed plans:
- macOS: [MAC_PACKAGING.md](MAC_PACKAGING.md) — phases M0-M7
  (Developer Program enrolment → notarised universal DMG → Tauri 2
  auto-updater → Homebrew Cask).
- Windows: [WINDOWS_PLAN.md](WINDOWS_PLAN.md) § W7 — OV code-signing
  cert + MSI bundle + Tauri 2 updater on the same Ed25519 channel.

Both sides share the auto-update Ed25519 signing key (single channel,
single `latest.json` per platform).

Cert procurement is the long-lead item on both sides:
- Apple Developer Program: 24-72 h enrolment.
- Windows OV cert: 1-4 weeks. Start in parallel with Phase 1b W3.

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
