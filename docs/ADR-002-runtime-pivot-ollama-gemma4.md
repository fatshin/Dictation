# ADR-002 — LLM runtime: pivot from `candle` to `Ollama` for Gemma-4

- **Status**: Proposed (2026-04-21)
- **Scope**: Phase 1 LLM inference path. Supersedes ADR-001 §LLM runtime.
  ASR (whisper-rs) is unchanged.
- **Driver**: User directed Gemma-4 (E2B/E4B/26B-A4B/31B Dense) as the
  primary LLM. candle 0.9.x has no Gemma-4 loader (PLE architecture
  unsupported). User explicitly named Ollama as the target runtime.
- **Supersedes**: ADR-001's `candle-transformers =0.9.2` selection.

## Context

Two days after ADR-001 (candle pivot), the user re-anchored model selection
on Gemma-4 — released 2026-04-02 with Per-Layer Embeddings (PLE),
audio-native input on E2B/E4B, and an Apache-2.0 license. The candle
runtime in 0.9.x has no Gemma-4 loader; only `quantized_gemma3.rs` exists,
and PLE is documented (Google AI Edge guidance) as "third-party
implementation has high cost; use official inference engines (Ollama,
LiteRT-LM)."

A 20-minute architecture survey (`docs/OSS_LANDSCAPE.md` updated)
evaluated runtimes that *do* support Gemma-4 today on the desktop targets
(macOS Metal, Windows x86 CUDA/CPU). Findings:

| Runtime | Gemma-4 day-0 | Cross-platform | Rust path | OSS Tauri precedent | Verdict |
|---|---|---|---|---|---|
| **Ollama** | Yes (4/2 tag) | macOS+Windows | HTTP via `ollama-rs` | Yes (OpenPawz) | **Selected** |
| llama-cpp-rs (`llama-cpp-2`) | Yes (GGUF live) | Yes | In-process FFI | Limited | **Fallback** |
| LiteRT-LM | Yes (official) | iOS-/Android-leaning; desktop weak | No Rust binding | None | Rejected (PoC risk) |
| MLX | Yes | macOS only | Swift/Python | n/a | Rejected (no Windows) |
| ONNX Runtime | Not confirmed | Yes | `ort` crate | n/a | Rejected (PLE export not found) |
| mistral.rs | Not confirmed for Gemma-4 | Yes | In-process | Small | Rejected (lag risk) |

## Decision

Replace `candle` with **Ollama** as the LLM runtime, embedded as a Tauri
`externalBin` sidecar. Communication: `reqwest` over `127.0.0.1:11434`
loopback. Models: Gemma-4 E2B (default fallback) and E4B (default primary)
only — 26B A4B and 31B Dense exceed the 8 GB RAM hard line at INT4 (~18 GB
and ~20 GB respectively) and are out of Phase-1 scope.

ASR remains `whisper-rs` (in-process), per ADR-001. Audio-native Gemma-4
is **not** activated in Phase 1 — the llama.cpp backend underneath Ollama
is text-stable but its audio path is in active development. Audio-native
becomes a Phase-2 evaluation.

## Consequences

### Benefits

- Gemma-4 E2B / E4B work day-0 on all three target environments (macOS
  Metal, Windows CUDA, Windows CPU). PLE is handled inside llama.cpp
  (which Ollama embeds), not in our code.
- Eliminates the per-architecture `Cache` reset and tensor-layout patching
  burden flagged by Codex review (Phi-4 fused QKV, Qwen3 cache clear,
  per-arch quirks). All hidden behind a stable HTTP API.
- Cuts the LLM-runtime line of code in our Rust backend by an order of
  magnitude. Mediator is a thin `reqwest` client around `/api/generate`.
- Apache-2.0 (Gemma-4) + MIT (Ollama) — clean for commercial distribution.
- Future model swaps are `ollama pull <model>` calls, not Cargo deps.

### Costs

- **Sidecar binary in distribution**: Ollama macOS bundle ~150 MB,
  Windows ~200 MB. Adds to installer size (was ~50 MB target). Mitigation:
  defer model bytes to first-run download; ship runtime only.
- **HTTP boundary** instead of in-process call. Latency overhead ~1-2 ms
  per request (irrelevant vs decode time). But it introduces a network
  surface that the security model has to explicitly carve out.
- **Apple sandbox loopback** is not perfectly characterized. macOS sandbox
  *does* allow `127.0.0.1` connections under `com.apple.security.network.client`,
  but the entitlement is the same as full internet access — there's no
  loopback-only entitlement. Mitigation: enforce egress denial at app
  layer (allowlist `127.0.0.1:11434` only), and validate with `nettop`
  during Phase 1 acceptance. **This is the largest unknown in this ADR.**
- **External process lifecycle**: ollama crashes have to be handled by a
  supervisor (similar to the original WhisperKit-CLI sidecar plan that
  ADR-001 retired).
- **Re-evaluation of all Phase-0 work**: candle Day-4/4.5 spike + the
  in-flight `candle-bench` run become reference data, not the gate
  numbers. The bench harness *pattern* (SQLite schema, prompt JSON
  single-source, audit fields) carries over and is reusable against
  Ollama with a thin shim.
- **Audio-native input is deferred**: even though Gemma-4 E2B/E4B support
  audio natively, llama.cpp's audio path is "active development" as of
  2026-04. Phase 1 stays text-only (whisper-rs → text → Ollama LLM).

### Phase-0 knock-on work

1. Remove `candle-bench` from Day-5 critical path. Keep the binary +
   collected rows as reference; mark them ADR-001 in the audit log.
2. Build a thin `ollama-bench` Rust binary that reuses the schema from
   `candle-bench` but POSTs to `/api/generate` with `temperature=0`,
   measures TTFT from request-sent → first SSE chunk, and tokens/sec
   from streamed event timestamps.
3. `models.py` adds an `OLLAMA_TIER_1` dict: `gemma4:e2b`, `gemma4:e4b`.
   GGUF download path retired in favor of `ollama pull` orchestration.
4. Re-run the 18-workload matrix on Ollama for E2B + E4B, then judge.
5. Aggregate, update `gate_decision_draft.md`.

## Rejected alternatives (with one-line reason each)

- **Stay on candle, drop Gemma-4** — user-directed primary model selection
  forecloses this option.
- **Patch candle to support Gemma-4 PLE** — multi-week implementation
  cost per Google's own guidance; out of Phase-0/1 budget.
- **LiteRT-LM** — no desktop Tauri integration precedent; PoC risk.
- **MLX** — Windows non-support; immediate disqualification.
- **llama-cpp-rs (in-process)** — viable but adds clang/CUDA toolchain
  to CI and we lose Ollama's auto-update / model-management ergonomics.
  Kept as documented fallback in case Ollama sidecar lifecycle proves
  unworkable.

## Open verification (Phase-1 entry blockers)

1. **Apple sandbox loopback** under hardened-runtime + App Sandbox: does
   `127.0.0.1:11434` work with *only* loopback intent and *no* general
   network egress? Validate with `nettop` showing zero non-loopback flows.
2. **Tauri 2 externalBin** packaging of the Ollama binary across macOS
   notarization and Windows code-signing.
3. **Ollama crash supervisor**: restart-on-exit with capped attempts;
   surface `LlmEvent::Crashed` to UI.
4. **Gemma-4 JP keigo quality** at E4B with a few-shot prompt — must
   exceed Phase-0 gate (`quality_avg ≥ 7.0`, all 4 axes ≥ 5.0).

## References

- ADR-001: `docs/ADR-001-runtime-pivot-candle.md` (now partially superseded)
- OSS landscape survey: `docs/OSS_LANDSCAPE.md`
- Ollama: https://ollama.com / https://github.com/ollama/ollama
- Gemma-4 (Google AI for Developers): https://ai.google.dev/gemma
- ollama-rs (Rust client): https://github.com/pepperoni21/ollama-rs
- Tauri 2 externalBin: https://v2.tauri.app/develop/sidecar/
- OpenPawz (Tauri 2 + Ollama precedent): https://github.com/OpenPawz/openpawz
