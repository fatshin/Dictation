# CI / Release workflows

Populated during Phase 1.

Planned:

- `ci.yml` — On PR: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `pnpm test`. Matrix: macOS (arm64) + Windows (x64, arm64).
- `release-mac.yml` — On tag `v*`: build universal `.dmg`, sign with Developer ID, notarize, staple, publish to GitHub Releases.
- `release-win.yml` — On tag `v*`: build MSIX x64 + ARM64, code-sign with SignTool, publish to GitHub Releases.
- `sidecar-build.yml` — On sidecar source change: build WhisperKit CLI (Swift) and download/verify `sherpa-onnx`, cache artifacts for release workflows.
