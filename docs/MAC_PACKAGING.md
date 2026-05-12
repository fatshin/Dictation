# Mac Distribution Plan

Companion to `ROADMAP.md` § Phase 4. The roadmap allocates "2-3 weeks"
for signed distribution; this document is the macOS-specific
decomposition.

## Current state (2026-05-12)

- Local development builds with `pnpm tauri dev`. Unsigned. Hardened
  runtime not enabled. Notarisation not configured.
- Bundled with `pnpm tauri build` produces a `.app` and a `.dmg`
  artifact under `src-tauri/target/release/bundle/`, but the binary
  is unsigned and macOS Gatekeeper blocks it with "cannot be opened
  because the developer cannot be verified."
- No CI publish pipeline. No auto-update channel.

To ship a public macOS build a user can double-click without
right-click-override, we need: Apple Developer ID (paid), code
signing, hardened runtime, entitlements, notarisation, and a
distribution channel.

## Tiers of distribution polish

| Tier | What the user sees | Effort |
|---|---|---|
| 0 (today) | Right-click → Open → "Are you sure?" → quarantine warning | 0 |
| 1 (Developer ID) | Double-click opens after a one-time "downloaded from internet" warning | Apple Developer Program ($99/yr) + ~3 days |
| 2 (Notarised) | Double-click opens cleanly, no warning | + 2-3 days |
| 3 (Auto-update) | Future versions install themselves | + 1 week |
| 4 (Brew cask) | `brew install --cask dictation` | + 1-2 days post-tier 3 |

Phase 4 in `ROADMAP.md` is tier 2 + tier 3. Tier 4 is a follow-up.

## Phase decomposition

### M0 — Developer Program enrolment (1-3 days lead time, async)

- Apple Developer Program enrolment ($99/yr individual). The
  enrolment can take 24-72 h for identity verification. Start this
  before any code work.
- Create the **Developer ID Application** certificate
  (`developer.apple.com/account/resources/certificates`). Note: this
  is NOT the App Store certificate — Mac App Store distribution is a
  separate path with its own sandboxing requirements (see "Why not
  Mac App Store" below).
- Install the cert in the Keychain on the build machine; export the
  `.p12` for CI.

**Exit**: `security find-identity -v -p codesigning` lists
"Developer ID Application: <name> (<team-id>)".

### M1 — Hardened runtime + entitlements (2-3 days)

The hardened runtime is required for notarisation. It denies
default behaviours (debugger attach, JIT, library loading from
arbitrary paths) unless an entitlement explicitly allows them.

Required entitlements for Dictation:

```xml
<!-- src-tauri/macos.entitlements -->
<key>com.apple.security.cs.disable-library-validation</key>
<true/>  <!-- needed if any unsigned dylibs are bundled (whisper-rs metallib) -->
<key>com.apple.security.device.audio-input</key>
<true/>
<key>com.apple.security.automation.apple-events</key>
<true/>  <!-- AppleScript for focus-detection polling -->
```

**Explicitly NOT added** (preserves the "no outbound network" property
in ADR-002 and CLAUDE.md):
- `com.apple.security.network.client` — would allow arbitrary internet
  connections. Loopback to `127.0.0.1:11434` works without this
  entitlement under App Sandbox; if we ever sandbox the app this
  needs re-evaluation.
- `com.apple.security.app-sandbox` — Dictation is **not sandboxed** in
  Phase 4. Sandboxing requires entitlements per accessed file and
  breaks the user's "save export to anywhere" expectation. Document
  the tradeoff in `docs/SECURITY.md`.

Configure `src-tauri/tauri.conf.json`:

```json
"bundle": {
  "macOS": {
    "hardenedRuntime": true,
    "entitlements": "macos.entitlements",
    "signingIdentity": "Developer ID Application: <name> (<team-id>)"
  }
}
```

**Exit**: `pnpm tauri build` produces a signed `.app` that passes
`codesign --verify --deep --strict` and `spctl --assess --type
execute` (the latter will still fail until notarisation, but the
verify check confirms the signature is well-formed).

### M2 — Notarisation (2-3 days)

Apple's notarisation service scans the binary for malware and signs
an approval ticket. Without it, macOS Gatekeeper shows the "developer
cannot be verified" warning even on a signed binary.

- Create an app-specific password at appleid.apple.com for the
  notarytool CLI.
- Store credentials in the keychain: `xcrun notarytool store-credentials`.
- Submit: `xcrun notarytool submit ./Dictation.dmg --keychain-profile <name> --wait`.
- Staple: `xcrun stapler staple ./Dictation.dmg` so notarisation
  works offline.

Iterate on the first submission. Common failures:

| Error | Cause | Fix |
|---|---|---|
| "The signature of the binary is invalid" | Inner bundle (e.g. WhisperKit.framework) unsigned | Sign with `codesign --deep` or sign inner bundles individually before the outer |
| "Hardened runtime not enabled" | tauri.conf.json hardenedRuntime not set | Re-bundle |
| "Invalid timestamp" | No `--timestamp` in codesign args | Tauri 2 bundler usually handles; verify |
| "Disallowed entitlement" | Entitlements file requested something the cert doesn't grant | Trim entitlements; some require `com.apple.developer.*` entitlements that need explicit Apple approval |

**Exit**: a fresh user on a fresh Mac downloads the DMG, opens it,
launches the app, no Gatekeeper warning.

### M3 — DMG layout polish (1-2 days)

- Custom DMG background image (`assets/dmg-background.png`) showing
  the "drag this icon → Applications" instruction.
- Window size, icon positions, volume name.
- Tauri 2 bundler supports DMG customisation via
  `bundle.macOS.dmg.background` and friends, or we can use the
  `create-dmg` shell tool for finer control.

**Exit**: opening the DMG presents the standard Apple "drag to
Applications" UX with brand visuals.

### M4 — Auto-update (1 week)

Two viable options:

| Option | Pros | Cons |
|---|---|---|
| **Tauri 2 built-in updater** | First-party; one config block; sig verification built-in | New (Tauri 2.x); fewer rough edges than Sparkle but smaller ecosystem |
| **Sparkle** (Swift framework) | Battle-tested on macOS (used by 1Password, Slack desktop pre-Electron) | Requires Swift integration in the Rust app; more moving parts |

Recommendation: **Tauri 2 updater** for v1.0, revisit Sparkle if the
update channel reliability falls short.

Configuration:

```json
"plugins": {
  "updater": {
    "active": true,
    "endpoints": ["https://github.com/fatshin/Dictation/releases/latest/download/latest.json"],
    "pubkey": "<base64-ed25519-public-key>"
  }
}
```

- Generate a long-lived Ed25519 signing key (separate from the code
  signing cert). Store the private half in Bitwarden / 1Password;
  store the public half in `tauri.conf.json`.
- The CI release pipeline (M5) signs each release with this key and
  produces the `latest.json` manifest.
- The app polls the endpoint on startup + every 24 h. UI shows an
  unobtrusive "Update available" notice; the user clicks to apply.
- No silent updates. Privacy-first apps respect user agency over
  what code runs locally.

**Exit**: a v1.0.0 user is prompted to update to v1.0.1 within 24 h
of release. The update verifies signature before installing.

### M5 — CI release pipeline (3-5 days)

- GitHub Actions workflow `.github/workflows/release.yml` triggered
  by tag push (`v*`).
- Matrix: `macos-14` (Apple Silicon native) and optionally
  `macos-13` (Intel). Single universal binary is preferable to ship
  one DMG: `cargo` build for `aarch64-apple-darwin` and
  `x86_64-apple-darwin`, `lipo` together.
- Secrets: `APPLE_CERTIFICATE` (.p12 base64), `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_NOTARY_PASSWORD`,
  `TAURI_PRIVATE_KEY` (updater Ed25519), `TAURI_KEY_PASSWORD`.
- Steps: build → codesign → notarytool submit → stapler → upload
  DMG + `latest.json` to GitHub Releases.

Total wall-clock time per release: ~15-25 min (notarisation latency
dominates).

**Exit**: `git tag v1.0.0 && git push --tags` produces a GitHub
release with a signed, notarised, stapled DMG and a signed
`latest.json` for the updater channel.

### M6 — Public download experience (2-3 days)

- Project README links to GitHub Releases.
- A static landing page (Cloudflare Pages / GitHub Pages) with:
  - One-click download (auto-detected arch).
  - Privacy claim ("no audio leaves your device") with
    verification recipe (`nettop -p $(pgrep -x dictation)`).
  - System requirements.
  - SHA-256 checksums for the DMG.
- Document the first-launch UX (consent dialog, AX permission
  prompt, mic permission prompt — order matters; AX must be granted
  first or focus-context falls back).

**Exit**: a stranger reading the README can download, install, and
record within 5 minutes.

### M7 — Homebrew Cask (1-2 days post-M6)

- Submit a cask to `homebrew/cask`:
  ```ruby
  cask "dictation" do
    version "1.0.0"
    sha256 "..."
    url "https://github.com/fatshin/Dictation/releases/download/v#{version}/Dictation_#{version}_universal.dmg"
    name "Dictation"
    desc "Local-first offline dictation with optional LLM rewrite"
    homepage "https://github.com/fatshin/Dictation"
    livecheck do
      url :url
      strategy :github_latest
    end
    auto_updates true   # Tauri updater handles in-app updates
    app "Dictation.app"
    zap trash: [
      "~/Library/Application Support/Dictation",
      "~/Library/Preferences/com.dictation.plist",
    ]
  end
  ```
- Cask review can take 1-2 weeks; not gating for the v1 release.

## Why not Mac App Store

- Mandatory sandbox breaks the `com.apple.security.device.audio-input`
  + Accessibility + focused-context combination cleanly. Each
  permission becomes a separate entitlement review.
- 30% Apple cut on optional paid features (none planned, but the
  policy applies if we ever monetise).
- Notarisation alone gives the security signal users need; the App
  Store badge is not worth the sandbox redesign for v1.

Revisit post-v1 if a clear distribution-channel benefit emerges.

## Risk register

| ID | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| MR-1 | Notarisation rejection on first submission | High | Low (a few iteration cycles) | Submit early in M1, not at release time |
| MR-2 | Apple Developer Program enrolment delay > 1 week | Med | High (blocks tier 1 entirely) | Start enrolment now; it costs $99 and 30 min of paperwork |
| MR-3 | Hardened runtime breaks whisper-rs Metal shaders | Med | High (no LLM rewrite if shaders fail) | Test M1 against the actual model load early; fall back to no-Metal CPU path if needed |
| MR-4 | Tauri 2 updater Ed25519 key compromise | Low | Critical (attacker can ship malicious update) | Store private key offline; rotate via point release if leaked |
| MR-5 | Signed binary still flagged by some enterprise EDR | Med | Low | Document the cert chain + thumbprint so IT teams can allow-list |
| MR-6 | Apple deprecates Developer ID for non-MAS distribution | Low | Critical (existential) | Track Apple platform announcements; ship a non-Apple-dependent escape hatch (Linux build path) before the deprecation grace window |

## Open decisions

1. **Universal binary vs separate Intel/ARM DMGs**. Universal doubles
   the download size (~80-120 MB total) but halves user confusion.
   Recommend universal for v1; revisit if size becomes a complaint.
2. **Updater channel layout**: latest-only or beta + stable channels?
   v1 ships single channel. Beta opt-in is a v1.1 feature.
3. **Crash reporting**: opt-in only, even when added later (CLAUDE.md
   "no telemetry without demonstrated need"). Need to design the
   "demonstrated need" gate.

## Acceptance criteria for Phase 4 (Mac side)

1. A fresh macOS 14 / 15 / 26 install can download the DMG, install,
   and launch Dictation without any Gatekeeper / quarantine prompt
   beyond the standard "downloaded from internet" disclosure.
2. `nettop -p $(pgrep -x dictation)` shows no non-loopback flows
   during a 60-second recording → rewrite → paste cycle.
3. `codesign --verify --deep --strict --verbose=2 Dictation.app`
   reports `valid on disk` and `satisfies its Designated Requirement`.
4. `spctl --assess --type execute --verbose Dictation.app` reports
   `source=Notarized Developer ID`.
5. A v0.9.x → v1.0.0 upgrade via the auto-updater is verified
   end-to-end on a test machine.
6. The README contains a one-line install instruction (`brew install
   --cask dictation` once M7 lands).

## References

- `docs/ROADMAP.md` § Phase 4 (the line item this expands)
- `docs/SECURITY.md` § Mac entitlements rationale (to be authored)
- ADR-002 § Apple sandbox loopback (entitlements interaction with
  Ollama)
- https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution
- https://v2.tauri.app/distribute/macos/
- https://v2.tauri.app/plugin/updater/
