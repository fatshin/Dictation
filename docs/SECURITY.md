# Security model

Local-first dictation handles sensitive material: meetings, medical notes, legal drafts, internal comms. The security model is designed to be auditable end-to-end. If you find a gap, please open a private security advisory.

## Threat model (STRIDE-lite)

| # | Threat | Severity | Primary mitigation |
|---|---|---|---|
| T1 | Lost laptop, FileVault/BitLocker off | High | Document: require full-disk encryption in README |
| T2 | Same-user malware reading DB | Critical | SQLCipher + OS keystore; per-session recording keys |
| T3 | TCC / UAC bypass (chained OS CVEs) | High | Require up-to-date OS; don't use disableable entitlements |
| T4 | Data leaks into Time Machine / File History / iCloud Drive | High | `isExcludedFromBackupKey`, `.nosync` directories, exclude from Spotlight |
| T5 | APFS snapshots / SSD remnants | High | Cryptographic erasure via per-session keys, not filesystem deletion |
| T6 | Memory / swap disclosure | Medium | mlock critical buffers where possible, zero on drop |
| T7 | Unintended outbound connections | Critical | Deny-all HTTP client factory; network entitlements not granted |
| T8 | Vulnerable dependencies (decoders, parsers) | High | SBOM + CVE scan in CI; sandboxed parsing for PDF/DOCX |
| T9 | Meeting-consent / legal-use concerns | Critical | Explicit consent UI before first recording; opt-in per session |
| T10 | Screen capture / clipboard scrape | Medium | Mitigation only (best-effort); documented limit |
| T11 | Export-path leaks | Critical | Export disabled by default; re-auth required to enable |

## Encryption at rest

**Scope**: protects the database from lost-laptop / offline-disk attacks and from other local users. **Not in scope**: same-user malware running with the logged-in user's privileges — once malware can impersonate the user, it can ask the OS keystore for the key and the app will decrypt as normal. Secure Enclave / TPM raise the cost of that attack but do not eliminate it.

- **SQLCipher 4.x**, with:
  - `cipher_page_size = 4096`
  - `kdf_iter ≥ 256000` (PBKDF2-HMAC-SHA512)
  - `cipher_memory_security = ON`
  - `PRAGMA temp_store = MEMORY`
  - Integrity check on open
- The database key is **not** stored as a user password. On first launch we generate a 32-byte random key and store it in:
  - macOS: Keychain item with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, wrapped via a Secure Enclave P-256 key when available (biometry-gated optional mode)
  - Windows: DPAPI with TPM-backed `NCrypt` key when the device has TPM 2.0
- The raw key never lives on disk in plaintext and never leaves process memory as a `String`.

## Network posture

Two build profiles to keep the audit story honest:

- **Audit build** (`cargo build --no-default-features`): no `reqwest`, no HTTP client, no downloader module. Compiles and runs against a pre-installed models directory. Used by anyone who wants to verify "no network code" from source.
- **Release build** (default features, what ships in the DMG/MSIX): includes the downloader module because models aren't bundled. The downloader has its own allowlist limited to model-host domains and is only invoked on explicit user action ("Download model X"). Once models are installed, dictation operation runs with zero egress — verifiable with `nettop` (macOS) or Resource Monitor (Windows).

CI asserts that `cargo tree --edges normal --no-default-features` does not include `reqwest`, so a future regression can't quietly re-introduce HTTP in the audit build.

The `reqwest` crate is feature-gated:

```toml
[dependencies]
reqwest = { version = "0.12", optional = true, default-features = false }

[features]
downloader = ["dep:reqwest"]
```

The model-downloader module is the only code path that pulls in `reqwest`, and it's gated behind an explicit user action ("Download model X"). Runtime allowlist limits destinations to model-host domains.

Tauri capabilities:

- `http:default` → **not granted**
- Shell access uses Tauri v2 sidecar permissions scoped to exact binaries (`whisperkit-cli-*`, `sherpa-onnx-*`). Arbitrary `shell:allow-execute` is **not** granted; argv is validated against a small list of known-safe arguments before the sidecar is spawned.

Runtime verification:

- On macOS, `nettop -m tcp -p <pid>` should show zero outbound connections during normal dictation
- On Windows, Resource Monitor → Network tab should show zero activity from the app
- Users are encouraged to run Little Snitch (Mac) or a hosts-file-based blackhole (any OS) to verify

## Entitlements (macOS)

Required:

- `com.apple.security.device.audio-input`
- `com.apple.security.automation.apple-events` (for accessibility-based text injection)

Explicitly **not** requested:

- `com.apple.security.network.client`
- `com.apple.security.network.server`
- `com.apple.security.files.user-selected.read-write`
- `com.apple.security.device.camera`
- `com.apple.security.personal-information.location`
- `com.apple.security.cs.disable-library-validation`
- `com.apple.security.cs.allow-dyld-environment-variables`
- `com.apple.security.cs.allow-unsigned-executable-memory`
- `com.apple.security.get-task-allow`

Hardened Runtime and App Sandbox are both enabled on release builds.

## Entitlements (Windows)

- MSIX package with AppContainer
- Capabilities: `microphone`, `runFullTrust` (required for UI Automation-based text injection)
- No `internetClient`, no `internetClientServer`

## Backup exclusion

On first run, the data directory is marked with:

- macOS: `URLResourceKey.isExcludedFromBackupKey` = true, directory name ends in `.nosync`, Spotlight index disabled (`.noindex`)
- Windows: `FILE_ATTRIBUTE_NOT_CONTENT_INDEXED`

## Cryptographic erasure

Recordings and live transcripts use a per-session ephemeral key. The key lives in Keychain/DPAPI only for the duration of the session and is destroyed on exit. This avoids reliance on filesystem delete semantics (APFS snapshots, SSD wear-leveling).

Persistent items (saved transcripts, rewrites) use the main DB key. Users who want forward-secrecy of old data can rotate the DB key, which re-encrypts in place.

## Logging discipline

- Unified Log / Event Log: no transcript content, no filenames, no user-identifiable strings
- Crash reports: stripped of user strings before local storage
- No opt-in crash reporting in v1.0; if added later, it will be off by default and payload will be reviewable before send

## Dependency hygiene

- `cargo audit` + `cargo deny` in CI
- SBOM (CycloneDX) generated per release and attached to GitHub release
- License scan: deny list for non-commercial (CC-BY-NC) and research-only licenses
- PDF/DOCX parsing (if added) runs in a separate process with limited capabilities

## Distribution integrity

- macOS: Developer ID signed, notarized, stapled
- Windows: code-signed release binaries; Authenticode signature verified by updater
- Updates: Sparkle (Mac) / Tauri updater (Win) verify signatures before applying

## Consent and legal

- First-launch onboarding requires a positive confirmation that the user is authorized to record the meetings they intend to process
- Per-session consent checkbox before recording starts
- Terms of Use explicitly state that the user is responsible for complying with local recording-consent laws (one-party / two-party jurisdictions)

## Known limits

- Screen capture: macOS ScreenCaptureKit can override window-level content protection. We cannot technically prevent screen recording; we document this and rely on OS-level MDM policies in enterprise use.
- IME caches: third-party IMEs with cloud sync may leak typed text outside the app. The onboarding flow recommends a local-only IME.
- OS-level ML features (Apple Intelligence, Writing Tools, predictive text) may surface input to the OS AI layer. We disable them at the text-view level where the API allows.

## Responsible disclosure

Security issues: please open a GitHub Security Advisory on this repo (private) before filing a public issue.
