# Architecture

## Layered view

```
┌───────────────────────────────────────────────────────────────┐
│ React + TypeScript (Vite)                                     │
│   - Main window (Settings, History)                           │
│   - Overlay window (floating dictation panel)                 │
│   - Tray menu                                                 │
│   - Zustand stores, TanStack Query for history search         │
├───────────────────────────────────────────────────────────────┤
│ Tauri 2 IPC layer                                             │
│   Commands: start_dictation, rewrite_text, search_history, …  │
│   Events:   asr:partial, asr:final, llm:token, model:progress │
├───────────────────────────────────────────────────────────────┤
│ Rust backend                                                  │
│   - asr/       (trait Asr; Mac WhisperKit sidecar / Win sherpa-onnx)
│   - llm/       (trait LlmRuntime; ort crate + KV-cache loop)  │
│   - db/        (trait EncryptedDb; rusqlite + sqlcipher)      │
│   - keystore/  (trait Keystore; Mac Keychain / Win DPAPI)     │
│   - audio/     (cpal + lock-free ring buffer)                 │
│   - hotkey/    (global-hotkey crate)                          │
│   - inject/    (trait TextInjector; enigo + AX / UIA)         │
│   - network_guard/ (deny-all HTTP client factory)             │
├───────────────────────────────────────────────────────────────┤
│ Platform sidecars                                             │
│   - macOS: WhisperKit CLI (Swift, ANE-accelerated)            │
│   - Windows: sherpa-onnx (C++, QNN/DirectML/CPU EPs)          │
├───────────────────────────────────────────────────────────────┤
│ ONNX Runtime + Execution Providers                            │
│   - macOS: CoreML, CPU                                        │
│   - Windows x64: DirectML, CPU                                │
│   - Windows ARM64 (Snapdragon X): QNN, DirectML, CPU          │
│   - Intel Core Ultra: OpenVINO                                │
│   - AMD Ryzen AI: Ryzen AI EP                                 │
└───────────────────────────────────────────────────────────────┘
```

## ASR sidecar wire protocol

Both Mac (WhisperKit CLI) and Win (sherpa-onnx) run as child processes and communicate over stdio using JSONL (newline-delimited JSON). This keeps the security boundary clean — no localhost sockets, no network policy conflict — and lets either sidecar crash independently without taking the app down.

Frame shapes (simplified):

```json
{"type":"start","session_id":"...","sample_rate":16000,"language":"ja"}
{"type":"pcm","seq":1,"data_b64":"..."}
{"type":"partial","text":"...","start_ms":0,"end_ms":1200}
{"type":"final","segments":[{"start_ms":0,"end_ms":3200,"text":"..."}]}
{"type":"error","code":"ASR_CRASH","message":"..."}
```

The Rust side owns the supervisor loop: spawn, monitor stdout, restart on exit, surface `AsrEvent::Crashed { reason }` to the UI when restart limit is hit. Audio capture never blocks on sidecar state — the `rtrb` ring buffer continues filling while the supervisor is reconnecting.

## Key trait boundaries (Rust)

```rust
#[async_trait]
pub trait Asr: Send + Sync {
    async fn start_stream(&mut self, cfg: AsrConfig) -> Result<AsrStream>;
    async fn transcribe_file(&self, path: &Path, cfg: AsrConfig) -> Result<Vec<Segment>>;
    fn capabilities(&self) -> AsrCapabilities;
}

#[async_trait]
pub trait LlmRuntime: Send + Sync {
    async fn load(&mut self, model_id: &str, ep: ExecutionProvider) -> Result<()>;
    async fn generate_streaming(
        &self,
        prompt: &str,
        params: GenParams,
        tx: mpsc::Sender<Token>,
    ) -> Result<GenStats>;
    fn unload(&mut self);
}

pub trait EncryptedDb: Send + Sync {
    fn open(path: &Path, key: &SecretKey) -> Result<Self> where Self: Sized;
    fn migrate(&mut self) -> Result<()>;
    fn transcripts(&self) -> &dyn TranscriptRepo;
    fn rewrites(&self) -> &dyn RewriteRepo;
}

pub trait Keystore: Send + Sync {
    fn get_or_create_db_key(&self, service: &str) -> Result<SecretKey>;
}

pub trait TextInjector: Send + Sync {
    fn inject(&self, text: &str, mode: InjectMode) -> Result<()>;
}
```

Platform impls are gated via `#[cfg(target_os = "...")]`. Front-end never sees the split.

## Three-window strategy

Tauri v2 `WebviewWindow`:

| Window | Role | Size | Opens on |
|---|---|---|---|
| `main` | Settings + History | 900×650 | Tray / menu |
| `overlay` | Floating dictation panel | 400×120, always-on-top, transparent | Hotkey press |
| `tray` | Native tray icon | — | Always on |

Overlay is created on demand and destroyed when dictation ends to minimize memory.

## IPC surface

~10 Tauri commands cover the full app. Representative signatures:

```rust
#[tauri::command]
pub async fn start_dictation(
    state: State<'_, AppState>,
    config: DictationConfig,
) -> Result<SessionId, String>;

#[tauri::command]
pub async fn rewrite_text(
    text: String,
    template_id: String,
    model_id: Option<String>,
) -> Result<RewriteJobId>;  // streams via event "llm:token"

#[tauri::command]
pub async fn list_models() -> Result<Vec<ModelInfo>>;

#[tauri::command]
pub async fn download_model(id: String) -> Result<DownloadJobId>;
```

Events flow Rust → frontend only:

- `asr:partial`, `asr:final` — streaming ASR segments
- `llm:token`, `llm:done` — streaming LLM output
- `model:download:progress` — bytes / total
- `hotkey:triggered`

## ONNX Runtime GenAI integration

**Phase 0**: `onnxruntime_genai` Python + ONNX Runtime smoke tests on each candidate. Python is chosen for speed of iteration — Phase 0 is about latency budget and quality ranking, not the production runtime.

**Phase 1 gate**: Before committing to the Rust backend for production, we verify that `ort` crate v2 can tokenize → prefill → decode 32 tokens on the chosen model. `onnxruntime-genai` has no official Rust binding (C/C++/C#/Java/Python only), so the Rust path is either:

1. `ort` v2 + manual KV-cache loop (primary), or
2. A thin FFI wrapper over the C API of `onnxruntime-genai` (fallback if (1) hits wall).

If neither works on the primary model at Phase 1 gate, fall back to `llama.cpp` + GGUF for the LLM layer. ASR is unaffected.

```rust
pub struct OnnxGenRuntime {
    session: ort::Session,
    tokenizer: tokenizers::Tokenizer,
    kv_cache: KvCache,
    ep: ExecutionProvider,
}
```

### Execution Provider selection

```rust
pub fn auto_select() -> ExecutionProvider {
    #[cfg(target_os = "macos")]
    { return try_coreml().unwrap_or(ExecutionProvider::Cpu); }

    #[cfg(target_os = "windows")]
    {
        if let Some(qnn) = try_qnn() { return qnn; }        // Snapdragon X
        if let Some(dml) = try_directml() { return dml; }   // GPU
        return ExecutionProvider::Cpu;
    }
}
```

### Model locations

```
macOS:   ~/Library/Application Support/Dictation/models/
Windows: %LOCALAPPDATA%\Dictation\models\

    └── <model-id>/
        ├── model.onnx
        ├── model.onnx.data
        ├── tokenizer.json
        └── genai_config.json
    └── MANIFEST.json          (hash + metadata)
```

MANIFEST.json carries SHA-256 for every file. The downloader verifies each file on arrival and refuses mismatches.

## Security boundary

**Tauri v2 capabilities** — `capabilities/main.json`:

```json
{
  "identifier": "main",
  "windows": ["main", "overlay"],
  "permissions": [
    "core:default",
    "fs:allow-app-read", "fs:allow-app-write",
    "dialog:allow-open",
    "shell:allow-execute",
    "notification:allow-all"
  ]
}
```

`http:default` is **not** granted. The `reqwest` dependency is feature-gated so the default build has no HTTP client at all. Model downloads go through a separately-gated module that is only compiled in when explicitly requested, and uses an explicit allowlist.

**macOS entitlements**:

- `com.apple.security.device.audio-input`
- `com.apple.security.automation.apple-events` (for text injection)
- Hardened Runtime + App Sandbox enabled
- `com.apple.security.network.client` **disabled**

**Windows**:

- MSIX + AppContainer
- Capabilities: `microphone`, `runFullTrust` (needed for UI Automation)
- `NCrypt` for TPM-backed key wrap

**SQLCipher key flow**:

1. First launch: generate 32-byte random key → store in Keychain (Mac) / DPAPI (Win)
2. Every launch: retrieve key → `PRAGMA key` on open
3. Key never appears on disk in plaintext

## Directory layout

Full tree is in [README.md](../README.md#project-layout-planned). Highlights:

- `src-tauri/` — Rust backend
- `src/` — React frontend
- `sidecars/` — Platform binaries (gitignored; built by CI)
- `models/` — Runtime model storage (gitignored; downloaded on first run)
- `research/phase0/` — Phase 0 benchmark scripts and evaluation data
- `scripts/` — Build, download, release helpers
- `.github/workflows/` — CI + release pipelines

## Open questions

1. Does Gemma 4 E4B have a working ONNX Runtime GenAI variant at PoC time? Fallback: drop to text-only Phi-4-mini / SmolLM3-3B.
2. Will CoreML EP be fast enough for 3–4B INT4 models? If not, we add an MLX sidecar path on macOS.
3. Is QNN EP stable for Phi-4-mini on Snapdragon X at INT4? Phase 0 benchmark will tell.
4. UIA text injection inside AppContainer — any path limitations? Prototype early.
