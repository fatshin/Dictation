# Windows Version — Development Plan

Companion to `ROADMAP.md` § Phase 1b. The roadmap line item allocates
"~2 weeks" for Windows parity; this document is the honest decomposition
showing why **7-9 weeks is the realistic figure for one developer** and
how the work breaks down.

## Targets

| Tier | OS | Arch | Why |
|---|---|---|---|
| 1 (must) | Windows 11 22H2+ | x86_64 | Largest install base for the target user (corp / consultant / law) |
| 1 (must) | Windows 11 22H2+ | ARM64 (Snapdragon X) | Strategic; Copilot+ PC fleet refresh underway through 2026 |
| 2 (best-effort) | Windows 10 22H2 | x86_64 | Support window ends 2025-10-14 + ESU 2028-10; not a primary target but should not crash |

ARM32 / Windows-on-Linux WSL / Server SKUs are explicit non-goals.

## Why this is not "just recompile"

A mechanical recap of the macOS-only surface in the current codebase
(`grep -rn 'cfg(target_os = "macos")'`):

| Module | macOS impl | Windows stub today | Real Windows impl needed |
|---|---|---|---|
| `hotkey/fn_key.rs` | CGEventTap on the fn modifier | `start_fn_key_listener` no-op | Low-level keyboard hook (`SetWindowsHookEx WH_KEYBOARD_LL`) on a chosen key (fn is not delivered to user space on most Windows keyboards) |
| `inject/context.rs` | `AXFocusedUIElement` + `AXValue` via accessibility-sys | Returns `None` | UI Automation (UIA) client: `IUIAutomation::GetFocusedElement` → `ValuePattern.Value` / `TextPattern` |
| `inject/focus.rs` | 250 ms osascript polling against frontmost app | Stub `false` | `GetForegroundWindow()` polling, compare against own HWND set |
| `keystore/macos.rs` | Keychain Services (`SecItem*`) | `StubKeystore` errors | DPAPI (`CryptProtectData`/`CryptUnprotectData`) for the SQLCipher key, with the per-user scope |
| `inject/mod.rs` `synth_paste` | Cmd+V via enigo on main thread | Already has `cfg(target_os = "windows")` branch for Ctrl+V | Verify SendInput timing on focused-window race |

Cross-platform pieces that need verification, not rewriting:
- `cpal` audio capture → WASAPI on Windows. Latency profile may differ.
- `whisper-rs` → Windows CPU build is supported; CUDA opt-in; **ARM64
  has no GPU/NPU acceleration today** (no QNN/DirectML in whisper-cpp
  upstream as of 2026-05).
- `rusqlite + bundled-sqlcipher` → cross-platform; needs `cmake` + a
  compatible C toolchain on the Windows runner.
- `enigo` → SendInput; works.
- `tauri-plugin-global-shortcut` → cross-platform alternative to the
  custom fn-key tap. Becomes the **primary trigger on Windows**.

## Phase decomposition

Each phase has a clear exit criterion. Items inside a phase can be
parallelised; phases themselves are sequenced because the later ones
depend on the earlier `cfg` scaffolding.

### W0 — Build green on Windows (2-3 days)

**Goal**: `cargo build` succeeds on `windows-2022` GitHub Actions runner.

- Add a Windows job to `.github/workflows/ci.yml` (matrix:
  `ubuntu-latest`, `macos-14`, `windows-2022`).
- Install dependencies: Rust msvc toolchain, `pnpm`, `Node 20`,
  Windows SDK, CMake, LLVM (for whisper.cpp).
- Resolve any `#[cfg]` gaps that surface only on Windows. Most stubs
  are in place; the build should pass with a minor amount of churn.
- Document the local dev path (Visual Studio Build Tools, MSVC, env
  vars).

**Exit**: green CI on a no-op Windows build of the existing tree.

### W1 — Whisper-only path runs end-to-end (1 week)

**Goal**: a developer can launch `pnpm tauri dev` on Windows, record
audio, see the Whisper transcript paste into Notepad.

- Verify `cpal` mic capture on Windows: pick the default WASAPI input,
  16 kHz / mono / f32 conversion.
- Verify `whisper-rs` Windows CPU build: ship `ggml-small.bin` in the
  same `models/` resolution path used on macOS.
- Verify `enigo` paste path under `cfg(windows)`: existing
  `synth_paste` already has a Ctrl+V branch.
- UI: Settings → bypass_llm ON path works (already implemented
  cross-platform in `App.tsx`; just needs a smoke test).

**Exit**: 5 s utterance → transcript visible in Output panel and
appears in clipboard after manual Stop button click.

### W2 — Global hotkey (3-5 days)

**Goal**: a key press from anywhere on the desktop starts/stops a
recording, same UX flow as macOS Cmd+Shift+D toggle.

The macOS fn long-press cannot be reproduced because fn is consumed
by keyboard firmware on most Windows hardware before reaching user
space. Design options:

| Option | Pros | Cons |
|---|---|---|
| `Win+Shift+Space` via `tauri-plugin-global-shortcut` | Already cross-platform; no new dependency | Some IMEs intercept; "Win" combos can collide with shell |
| `Ctrl+Alt+D` toggle | Universal; rarely conflicts | No push-to-talk feel |
| `Pause/Break` push-to-talk | Dedicated key on many keyboards | Absent on tenkeyless and laptop keyboards |
| Right-Ctrl held down as PTT | Mirrors fn long-press feel | Requires a low-level hook; some keyboards remap |

Decision: ship `Ctrl+Alt+D` as the default toggle and add **right-Ctrl
PTT via low-level keyboard hook** as an opt-in for users who want the
fn-long-press analogue.

- Implement `hotkey/win_hook.rs` with `SetWindowsHookEx(WH_KEYBOARD_LL)`
  on a background thread; emit the same `hotkey:press_start` /
  `hotkey:press_end` Tauri events the macOS fn-tap uses.
- Reuse the existing frontend listeners verbatim (already wired in
  `App.tsx`).

**Exit**: both Ctrl+Alt+D toggle and right-Ctrl PTT trigger the
dictation pipeline regardless of focused window.

### W3 — UI Automation for focused-field context (1-2 weeks)

**Goal**: when the user dictates while focused on a text field, the
LLM rewrite can see the surrounding text as context, same as macOS
`AXValue` extraction.

- Add `windows` crate (the official Microsoft `windows-rs`) with the
  `Win32_UI_Accessibility` feature.
- Implement `inject/context_win.rs`: COM-init on the worker thread,
  `CUIAutomation::GetFocusedElement` → try `ValuePattern.Value` first,
  fall back to `TextPattern.GetSelection` → `TextRange.GetText`.
- Truncate at 4096 chars like macOS does.
- Handle the COM threading model (`Manager.with_thread()` pattern;
  UIA must be called from the same thread that initialised COM with
  `CoInitializeEx`).

Known foot-guns:
- Electron / WebView / Chromium apps expose accessibility through a
  separate accessibility tree. UIA reaches it via `RawViewWalker` but
  some apps cache aggressively.
- Microsoft 365 apps have their own UIA quirks (delayed property
  notifications).
- Some apps require the elevated AccessibilityEvents permission;
  document degraded mode (context unavailable → ASR-only fallback,
  same as macOS without AX consent).

**Exit**: typing into Notepad / Slack / VS Code / Outlook with
selected text → recording fires → the rewrite prompt includes the
surrounding text via the existing `{context}` placeholder.

### W4 — DPAPI keystore (3-5 days)

**Goal**: SQLCipher's per-user key is sealed by Windows credential
guard, equivalent to the macOS Keychain integration.

- Add `keystore/windows.rs` using `windows::Win32::Security::Cryptography::DataProtection`.
- `CryptProtectData(plaintext_key, "Dictation DB key", null, null, null, CRYPTPROTECT_LOCAL_MACHINE=0)` → blob; store the blob in `%APPDATA%\Dictation\keystore.bin`.
- `CryptUnprotectData` on app start to recover the key.
- TPM-backed sealing (Windows Hello / `KeyCredentialManager`) is a
  follow-up; DPAPI alone is acceptable for Phase 1b because the user
  account already gates DB access at the OS level.

**Exit**: a fresh user launches the app, gives consent, records, and
on relaunch the same encrypted DB opens without prompting.

### W5 — Focus tracking + arm-and-paste (3-5 days)

**Goal**: after a rewrite completes, the result is on the clipboard;
the moment the user gives focus to another app, Ctrl+V fires there,
matching the macOS `focus:external` behaviour.

- Implement `inject/focus_win.rs`: 250 ms `GetForegroundWindow()`
  poll comparing against the app's HWND set (main window + overlay).
- Emit `focus:external` Tauri event on transition, identical payload
  contract as macOS.
- Reuse the frontend listener already wired in `App.tsx`.

**Exit**: record → Stop → switch to Notepad → text auto-pastes
without further action. Equal feel to macOS.

### W6 — LLM rewrite (opt-in, 1 week)

**Goal**: users who want the LLM rewrite path can pull
`gemma4:e2b` (pending ADR-005 verification) on Windows and have the
existing Rewrite pipeline work.

- Verify Ollama Windows x64 (.msi) integration: same loopback URL
  `127.0.0.1:11434`; no code change expected in `llm/`.
- Verify Ollama Windows ARM64: as of 2026-05 there is an official
  ARM64 build (preview). Confirm `qwen3.5:4b-q4_K_M`,
  `gemma4:e2b` runtime parity.
- **Block**: Windows ARM64 has no GPU/NPU acceleration in Ollama
  today; tokens-per-second on Snapdragon X CPU may not meet UX
  budget. Document the fallback (Whisper-only is the default UX on
  ARM64 until QNN/DirectML lands).
- Network-guard verification on Windows: `Get-NetTCPConnection` /
  `netstat -ab` shows only loopback flows during operation. Document
  the test in `docs/SECURITY.md`.

**Exit**: on Windows x64 with Ollama installed, the bypass=false path
produces a rewrite within the same TTFT budget as macOS (subject to
ADR-005 unblocking).

### W7 — Packaging + code signing (1-2 weeks)

**Goal**: a notarised-equivalent installer that ordinary users can
double-click without Defender SmartScreen warnings.

- Choose installer format: **MSI** for Phase 1 (lowest friction,
  enterprise-friendly), reserve MSIX for a Store-distributed channel
  later.
- Acquire an **OV code-signing certificate** (Sectigo or SSL.com,
  approx ¥35-50 k / yr). EV cert (~¥80-120 k / yr) bypasses
  SmartScreen warning immediately but is overkill for v1.
- Configure Tauri 2 bundler: `bundle.targets = ["msi"]`,
  `bundle.windows.certificateThumbprint`, `bundle.windows.timestampUrl`.
- SmartScreen reputation: an OV-signed binary needs ~25-100 downloads
  to build reputation. Document the expected first-user friction.
- Reproducible build pipeline on GitHub Actions: signed binaries
  generated from a tag push, secrets via GitHub OIDC + Azure Key
  Vault (or sign on a self-hosted runner with the cert physically
  attached if you want to avoid cloud key custody).

**Exit**: `dictation-1.0.0-x64.msi` and `dictation-1.0.0-arm64.msi`
download from GitHub Releases, install cleanly on a fresh Windows VM,
launch without admin elevation.

### W8 — Cross-platform regression pass (3-5 days)

**Goal**: every feature listed in the macOS feature inventory works
on Windows, with platform-specific UX differences explicitly
documented.

- Run the existing Playwright E2E suite (`e2e/regression.spec.ts`) on
  the Windows runner. UI tests should pass unchanged; OS-integration
  tests (focus, paste, hotkey) need manual verification.
- Author a Windows-specific test journey doc:
  `docs/test_journeys/windows.md` listing the 10-15 manual checks
  with screenshots.

**Exit**: feature parity matrix in `docs/PARITY.md` shows green for
all P0 items.

## Risk register

| ID | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| WR-1 | Windows ARM64 NPU/GPU not exposed to whisper.cpp / Ollama | High | Med (Snapdragon X users get CPU-only; latency target may slip) | Ship Whisper-only as default on ARM64; track upstream QNN backend; revisit when DirectML/QNN lands |
| WR-2 | OV code-signing cert procurement delay | Med | Med (delays signed release) | Start cert acquisition in parallel with W3; budget 2-4 weeks lead time |
| WR-3 | UIA inconsistency across apps (Electron, classic Win32, UWP) | High | Med (context feature degraded in some apps) | Same degradation policy as macOS: context unavailable → ASR-only fallback, no error |
| WR-4 | SmartScreen friction even with OV cert | High | Low | "Run anyway" docs in first-run wizard; build reputation organically |
| WR-5 | DPAPI re-prompts on user profile move / domain migration | Low | High (DB key loss = history loss) | Document the recovery path: `DICTATION_DB_KEY` env-var override; warn before destructive operations |
| WR-6 | Tauri 2 plugin-global-shortcut conflicts with corporate IT policies | Med | Low | Make the binding user-configurable from Settings; fall back to PTT hook |
| WR-7 | Windows 11 24H2 has not yet shipped during Phase 1b | Low | Low | Test against 23H2 + 22H2; track 24H2 ISO when available |

## Open decisions (require an owner before W2)

1. **fn-key analogue**: confirm right-Ctrl PTT vs Pause/Break vs Caps
   Lock. UX testing in W2.
2. **Code-signing strategy**: OV vs EV. Budget question, not technical.
3. **ARM64 minimum spec**: Snapdragon X Plus (8-core) baseline or also
   target Snapdragon X1 (6-core, Surface Laptop entry SKU)? Affects
   benchmark targets in W6.
4. **Installer artefact name**: `dictation-1.0.0-x64.msi` vs
   `Dictation_1.0.0_x64.msi`. Pick one consistent with the Mac DMG
   convention (`MAC_PACKAGING.md`).

## Honest schedule

Phase 1b in `ROADMAP.md` is "~2 weeks". That figure assumes the
Windows-side modules already exist as more than stubs and only need
hardening. The honest decomposition above sums to **7-9 weeks for one
person**, distributed:

| Phase | Effort |
|---|---|
| W0 | 2-3 d |
| W1 | 1 wk |
| W2 | 3-5 d |
| W3 | 1-2 wk |
| W4 | 3-5 d |
| W5 | 3-5 d |
| W6 | 1 wk |
| W7 | 1-2 wk |
| W8 | 3-5 d |
| **Total** | **7-9 wk** |

The bulk of the variance is W3 (UIA learning curve) and W7 (cert
procurement). Both have lead-time levers; start them earlier than the
dependency graph requires.

## Acceptance criteria for Phase 1b GA

1. The CI matrix is green on `ubuntu-latest`, `macos-14`,
   `windows-2022` x64 and ARM64.
2. The 11-test Playwright regression suite passes on Windows.
3. A signed MSI installs on a fresh Windows 11 22H2 VM without admin.
4. The Whisper-only golden path (record → paste in Notepad) works on
   x64 and ARM64.
5. The arm-and-paste flow works against at least Notepad, Word,
   Outlook, Chrome, Slack desktop, and VS Code.
6. No outbound network connections during a recording session, as
   verified by `netstat -an` showing only loopback `127.0.0.1:11434`
   (or zero connections in Whisper-only mode).
7. `docs/PARITY.md` is published with a green row per P0 item.

## References

- `docs/ROADMAP.md` § Phase 1b (the line item this document expands)
- `docs/ARCHITECTURE.md` § cross-platform abstractions
- `docs/SECURITY.md` § Windows-specific egress verification
- ADR-001 § Costs (Snapdragon X / Windows-ARM NPU caveat)
- ADR-002 § Open verification (Ollama Windows sidecar packaging)
- `src-tauri/src/{hotkey,inject,keystore}/` — existing macOS impls
  and Windows stubs that this plan turns into real code
