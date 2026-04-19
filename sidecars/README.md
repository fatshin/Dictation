# Sidecars

Platform-specific ASR binaries. Not committed; built by CI or via `scripts/build_sidecar_*.sh`.

## macOS

`whisperkit-cli` — a Swift CLI wrapping [WhisperKit](https://github.com/argmaxinc/WhisperKit). Built for both `aarch64-apple-darwin` and `x86_64-apple-darwin` and lipo'd into a universal binary.

Expected names at runtime (resolved by Tauri sidecar convention):

```
sidecars/whisperkit-cli-aarch64-apple-darwin
sidecars/whisperkit-cli-x86_64-apple-darwin
```

Source: `sidecar-src/whisperkit-cli/` (added in Phase 1).

## Windows

`sherpa-onnx` — prebuilt binary from [k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). Downloaded by `scripts/download_sidecars.ps1` and hash-verified.

Expected names at runtime:

```
sidecars/sherpa-onnx-x86_64-pc-windows-msvc.exe
sidecars/sherpa-onnx-aarch64-pc-windows-msvc.exe
```

## Verification

Every sidecar's SHA-256 is pinned in `sidecars/MANIFEST.json` (generated at Phase 1). Release builds refuse to bundle a sidecar whose hash doesn't match.
