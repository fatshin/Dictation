# Contributing

Pre-PoC. Structure-only contributions are welcome once Phase 0 completes.

## Ground rules

- Keep it local. No code that adds outbound network calls unless gated behind an explicit, opt-in capability (e.g., the model downloader).
- Keep it small. If a feature needs more than ~500 lines of Rust, open a design issue first.
- Keep it auditable. Anything that touches audio, text, or crypto must be reviewable by an experienced maintainer and covered by tests.

## Scope of acceptable PRs

Welcome:

- Bug fixes in ASR / LLM / DB / keystore / hotkey / injection code
- Cross-platform parity (e.g., making something that works on Mac also work on Windows)
- i18n (the UI currently assumes English + Japanese; other languages are welcome)
- New benchmark cases in `research/phase0/inputs/`
- Documentation improvements

Out of scope:

- Cloud sync, accounts, telemetry (see [ROADMAP.md](ROADMAP.md#non-goals))
- New model providers that require an account or an API key
- Pronunciation scoring / language learning features

## Dev environment

Not ready yet. After Phase 0:

```
rustup target add aarch64-apple-darwin x86_64-apple-darwin    # macOS
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc   # Windows
cargo install tauri-cli --version "^2.0"
pnpm install
pnpm tauri dev
```

## Commit conventions

- Conventional Commits: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, `perf:`
- Keep the subject ≤ 72 chars
- Explain *why* in the body if the change is non-obvious
- One logical change per commit

## Tests

Phase 0: benchmark scripts only.

Phase 1 onward:

- Rust unit tests in-tree (`#[cfg(test)]`)
- Rust integration tests under `tests/`
- E2E via Playwright for the UI

Minimum 80 % coverage for new Rust modules touching audio, text, or crypto.

## Security

Report security issues privately via GitHub Security Advisories on this repo. Do not file public issues for vulnerabilities.

## License

By contributing, you agree that your contributions are licensed under the MIT License.
