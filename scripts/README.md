# Scripts

Build / download / release helpers. Populated during Phase 1.

Planned scripts:

| Script | Platform | Purpose |
|---|---|---|
| `download_models.sh` | macOS / Linux | Fetch ONNX models + verify SHA-256 |
| `download_models.ps1` | Windows | Same, PowerShell |
| `download_sidecars.sh` | macOS / Linux | Fetch / build sidecar binaries |
| `download_sidecars.ps1` | Windows | Same |
| `build_mac.sh` | macOS | `cargo tauri build` + codesign + notarize |
| `build_win.ps1` | Windows | `cargo tauri build` + signtool |
| `build_sidecar_mac.sh` | macOS | Swift build of WhisperKit CLI |
| `verify_offline.sh` | any | Run the app and confirm zero egress via `nettop` / Resource Monitor |
| `bench.sh` | any | Re-run Phase 0 benchmarks |
