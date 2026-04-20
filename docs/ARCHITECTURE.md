# Architecture

## Layered view

> **Revised 2026-04-21** per ADR-001 (`docs/ADR-001-runtime-pivot-candle.md`).
> LLM runtime is now `candle`, not `ort`/onnxruntime-genai. ASR is unified on
> `whisper-rs` across macOS and Windows. The ASR-sidecar section and the
> `ONNX Runtime GenAI integration` section below remain for audit but are
> **superseded** by ADR-001.

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
│   - asr/       (trait Asr; whisper-rs + cpal + vad-rs)        │
│   - llm/       (trait LlmRuntime; candle-transformers, GGUF)  │
│   - db/        (trait EncryptedDb; rusqlite + sqlcipher)      │
│   - keystore/  (trait Keystore; Mac Keychain / Win DPAPI)     │
│   - audio/     (cpal + lock-free ring buffer)                 │
│   - hotkey/    (global-hotkey crate)                          │
│   - inject/    (trait TextInjector; enigo + AX / UIA)         │
│   - network_guard/ (deny-all HTTP client factory)             │
├───────────────────────────────────────────────────────────────┤
│ Inference backends (in-process, no sidecar)                   │
│   - whisper-rs (whisper.cpp binding)                          │
│   - candle-transformers 0.9.x (Phi-4-mini / Gemma / Qwen /    │
│     Llama GGUF, pinned)                                       │
├───────────────────────────────────────────────────────────────┤
│ Hardware acceleration                                         │
│   - macOS: Metal (candle) + Metal/CoreML (whisper.cpp)        │
│   - Windows x86 w/ NVIDIA: CUDA (candle) + CUDA (whisper.cpp) │
│   - Windows x86 CPU: CPU (both)                               │
│   - Windows ARM (Snapdragon X): CPU only — candle has no      │
│     QNN/DirectML backend as of 0.9.x. See ADR-001 for the     │
│     tradeoff and deferred-decision on NPU-on-Windows-ARM.     │
└───────────────────────────────────────────────────────────────┘
```

## ASR sidecar wire protocol (superseded by ADR-001; retained for audit)

> **Superseded.** Post ADR-001, ASR runs in-process via `whisper-rs`
> (whisper.cpp Rust binding). The JSONL-over-stdio protocol below applied
> to the WhisperKit-CLI and sherpa-onnx sidecar designs and is kept only
> so archaeology on pre-pivot branches remains legible.

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

## LLM runtime (candle, GGUF)

Post ADR-001 the LLM layer uses `candle-core` + `candle-transformers`,
pinned to `=0.9.2` to match sumi's lock and keep the blast radius of
candle's API churn bounded.

- Model format: GGUF (Q4_K_M or Q4_0 quantization targeting INT4 footprint).
- Loaders: candle's `quantized_phi3.rs` / `quantized_gemma3.rs` /
  `quantized_qwen3.rs` / `quantized_llama.rs`. Phi-4 requires a patched
  loader that splits fused QKV / gate+up tensors (reference:
  `alan890104/sumi` `src/models/phi4.rs` — GPL, design only).
- KV-cache: per-model `Cache` struct; manual-but-scoped. No `onnxruntime-genai`
  auto-management but no `ort`-manual-implementation effort either.
- Streaming: `LogitsProcessor` + greedy/temperature sampler in a loop; emit
  tokens over `mpsc::Sender<Token>` as before.

```rust
pub struct CandleRuntime {
    device: candle_core::Device,  // Metal / Cuda / Cpu
    model: Box<dyn TextGenerator>,
    tokenizer: tokenizers::Tokenizer,
    cache: KvCacheHandle,
}
```

### Device selection

```rust
pub fn auto_select() -> candle_core::Device {
    #[cfg(target_os = "macos")]
    { return candle_core::Device::new_metal(0).unwrap_or(candle_core::Device::Cpu); }

    #[cfg(target_os = "windows")]
    {
        if let Ok(cuda) = candle_core::Device::new_cuda(0) { return cuda; }
        // Windows ARM (Snapdragon X) → CPU only. candle 0.9 has no QNN
        // / DirectML / Vulkan backend. Documented in ADR-001.
        return candle_core::Device::Cpu;
    }
}
```

### Model locations

```
macOS:   ~/Library/Application Support/Dictation/models/
Windows: %LOCALAPPDATA%\Dictation\models\

    └── <model-id>/
        ├── model.gguf
        └── tokenizer.json
    └── MANIFEST.json          (SHA-256 + metadata)
```

MANIFEST.json carries SHA-256 for every file. The downloader verifies each
file on arrival and refuses mismatches. The old `model.onnx` + `.onnx.data`
+ `genai_config.json` triple is retired.

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

1. **Windows ARM (Snapdragon X)** has no candle NPU backend. Before Phase
   1b start, decide: (a) ship degraded CPU-only experience on Snapdragon,
   (b) dual-runtime with `onnxruntime-genai` QNN *just* for Windows ARM,
   or (c) defer Windows ARM to a later phase. See ADR-001 §Costs.
2. **candle API churn**: track `candle-transformers` release notes; the
   0.9 → 0.10 bump is a likely Phase-1-mid churn point. Pin is `=0.9.2`.
3. **Phi-4 GGUF loader**: sumi's patched loader is GPL-licensed. We
   re-implement the fused-tensor split in our own MIT/Apache code rather
   than copying. Budget: ~1 day.
4. **candle model availability for Japanese**: validate Gemma 3 / Phi-4-mini
   GGUF produce acceptable JP keigo on Day-4 bench (smoke suggested Phi-4
   does, Llama-3.2 did not).
5. **UIA text injection inside AppContainer** — any path limitations?
   Prototype early.
