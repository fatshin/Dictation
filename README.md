# Dictation

**Free, local-first AI dictation app. Your voice never leaves your machine.**

Cross-platform (macOS + Windows) voice dictation tool with on-device speech recognition and local LLM post-processing. Built for people who handle sensitive material — meetings, medical notes, legal drafts, internal communications — where sending audio to third-party clouds is not an option.

> **Status**: Pre-PoC. Architecture and Phase 0 plan are published here for review. Code lands after Phase 0 go/no-go.

---

## Why another dictation app?

| Existing tool | Problem |
|---|---|
| Superwhisper, Wispr Flow, Typeless | Subscription, cloud processing, or unverifiable privacy claims |
| MacWhisper | macOS only, thin LLM post-processing |
| Whispering (OSS) | No Japanese business-register rewriting, rough UX |
| OS-native (macOS Dictation, Windows Voice Access) | Weak rewriting, no cross-app hotkey workflow |

This project targets the gap: **auditable source code + fully offline + Japanese business register + Windows/macOS parity**.

---

## Design goals

1. **Free to run forever.** No subscription. No phone-home. No mandatory account.
2. **Local-first.** All ASR and LLM inference happens on your machine. No outbound network calls by default.
3. **Auditable.** Source code is public. You can verify with Little Snitch / Wireshark / PacketCapture that nothing leaves the device.
4. **Cross-platform.** Single codebase for macOS (Apple Silicon) and Windows (x64 / ARM64).
5. **Japanese + English first-class.** Mixed-language dictation, business-register rewriting, custom vocabulary.

---

## Architecture (planned)

```
┌───────────────────────────────────────────────────────────────┐
│ Tauri 2 Shell (Rust backend + React/TS frontend)              │
│                                                               │
│  Hotkey ─▶ Audio Capture ─▶ ASR ─▶ LLM Rewrite ─▶ Injection   │
│                             │       │                         │
│                    Mac: WhisperKit  │ ONNX Runtime GenAI      │
│                    Win: sherpa-onnx │ (CoreML / DirectML /    │
│                                     │  QNN / OpenVINO EP)     │
│                                                               │
│  Encrypted local DB (SQLCipher + OS-level key storage)        │
│  Network guard: no outbound connections by default            │
└───────────────────────────────────────────────────────────────┘
```

### Tech stack

| Layer | Choice | Rationale |
|---|---|---|
| UI shell | Tauri 2 | Small bundle, Rust backend, cross-platform |
| Frontend | React + TypeScript + Zustand | Widely known, fast iteration |
| LLM runtime | ONNX Runtime GenAI (`ort` crate) | NPU support on Win (QNN/OpenVINO/DirectML), CoreML on Mac, single model format |
| ASR (macOS) | WhisperKit (Swift sidecar) | Apple Neural Engine acceleration |
| ASR (Windows) | sherpa-onnx | QNN/DirectML EP, Whisper large-v3-turbo ONNX |
| Encryption | SQLCipher + Keychain/Secure Enclave (Mac), DPAPI + TPM (Win) | OS-native key storage |
| Hotkey | `global-hotkey` crate | Cross-platform |
| Text injection | `enigo` + platform accessibility APIs | Universal app support |
| Audio capture | `cpal` + lock-free ring buffer | Low-latency PCM |

### Candidate LLMs (to be benchmarked in Phase 0)

All models are small (≤4B parameters) and quantized to INT4 ONNX so they run on consumer laptops.

| Model | Size | License | Notes |
|---|---|---|---|
| Gemma 4 E4B | 4B | Gemma Terms of Use | Native audio input capability; license requires review before bundling |
| Gemma 4 E2B | 2B | Gemma Terms of Use | Lightweight variant |
| Phi-4-mini-instruct | 3.8B | MIT | First-class NPU variants (QNN / OpenVINO / Ryzen AI) |
| SmolLM3-3B | 3B | Apache 2.0 | 128K context for long summarization |
| Llama 3.2 3B | 3B | Llama 3.2 License | Fallback, Meta-quantized ONNX available |
| Qwen3 4B | 4B | Apache 2.0 | OpenVINO NPU reference path |

Model files are downloaded on first run; none are embedded in the binary.

---

## Project layout (planned)

```
Dictation/
├── src-tauri/              Rust backend
│   ├── src/
│   │   ├── asr/            ASR abstraction (Mac/Win impls)
│   │   ├── llm/            ONNX Runtime GenAI wrapper
│   │   ├── db/             SQLCipher + migrations
│   │   ├── keystore/       Keychain / DPAPI
│   │   ├── audio/          Recorder, ring buffer
│   │   ├── hotkey/         Global hotkey handling
│   │   ├── inject/         Text injection (enigo + AX / UIA)
│   │   └── network_guard/  Outbound-block policy
│   └── tauri.conf.json
├── src/                    React frontend (Vite)
├── sidecars/               Platform-specific binaries (WhisperKit CLI, sherpa-onnx)
├── models/                 Model files (gitignored, downloaded at install)
├── research/phase0/        Phase 0 benchmark scripts
├── scripts/                Build / download / release helpers
└── .github/workflows/      CI + release pipelines
```

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## Roadmap

| Phase | Scope | Status |
|---|---|---|
| 0 | Technical PoC — benchmark 4–6 candidate LLMs on Mac + Windows, choose primary + fallback, validate TTFT budget | Planning |
| 1 | MVP — Tauri 2 shell, ASR integration, LLM rewrite, encrypted local storage, global hotkey, text injection | — |
| 2 | Mixed-language handling, custom vocabulary, per-app tone switching | — |
| 3 | Meeting-file import, long-context summarization, history search | — |
| 4 | Signed distribution (notarized DMG / MSIX), auto-update, public release | — |

See [docs/ROADMAP.md](docs/ROADMAP.md) and [docs/PHASE0_POC.md](docs/PHASE0_POC.md).

---

## Privacy & security posture

- **No outbound connections.** The app ships with the network-client entitlement disabled (macOS) and no `http:*` Tauri capability. You can verify with Little Snitch or `nettop` that nothing is sent.
- **On-disk encryption.** All transcripts and rewrites are stored in a SQLCipher database. The database key lives in the OS keystore (Keychain + Secure Enclave on macOS, DPAPI + TPM on Windows) and never touches plaintext disk.
- **Per-session keys.** When enabled, recording buffers use a per-session ephemeral key that is discarded on exit — cryptographic erasure rather than relying on filesystem deletion.
- **Minimal entitlements.** Microphone + accessibility (for injection). No camera, no location, no contacts, no full-disk access.
- **Hardened Runtime + App Sandbox** on macOS. **AppContainer** on Windows.
- **No telemetry.** If opt-in crash reporting is added later, it will be off by default and will never include transcript content.

---

## License

- **Source code**: [MIT License](LICENSE)
- **LLM models** (downloaded on first run) retain their respective licenses:
  - Whisper: MIT
  - Phi-4-mini: MIT
  - SmolLM3: Apache 2.0
  - Qwen3: Apache 2.0
  - Llama 3.2: Llama 3.2 Community License
  - Gemma 4: Gemma Terms of Use (review before distribution)

Users are responsible for complying with the license of whichever model they download.

---

## Contributing

Early stage. Issues and discussion are welcome once Phase 0 completes. PRs should target the OSS core (ASR, LLM runtime, UI, i18n, platform support).

---

## Acknowledgements

- [Whisper](https://github.com/openai/whisper) by OpenAI
- [WhisperKit](https://github.com/argmaxinc/WhisperKit) by Argmax
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) by k2-fsa
- [ONNX Runtime GenAI](https://github.com/microsoft/onnxruntime-genai) by Microsoft
- [Tauri](https://tauri.app/)
- [Whispering](https://github.com/epicenter-md/epicenter) — inspiration for the fully-OSS dictation approach
