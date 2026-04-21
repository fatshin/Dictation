//! candle-bench: Phase-0 Day-5 LLM bench harness on candle Metal/CPU/CUDA.
//!
//! Schema-compatible with `bench_llm.py`: writes the same `bench_runs`
//! table in the same SQLite file, append-only, with bench_session_id
//! per invocation. Codex review (Day 4.5) drove the design — KV-cache
//! reset per measured run via fresh model load, full-prompt prefill at
//! pos=0, TTFT measured as prefill-forward-start → first-token-sampled,
//! tokens_per_sec across decode-only window, peak RAM sampled at 50ms
//! cadence in a background thread.
//!
//! Prompt templates loaded from `prompt_templates/<version>.json` so
//! Python and Rust share a single source.

use anyhow::{anyhow, Context, Result};
use candle_core::{quantized::gguf_file, Device, IndexOp, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::{
    quantized_gemma3 as gemma3, quantized_llama as llama, quantized_phi3 as phi3,
    quantized_qwen3 as qwen3,
};
use clap::{Parser, ValueEnum};
use rusqlite::{params, Connection};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokenizers::Tokenizer;
use uuid::Uuid;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Arch {
    Phi3,
    Llama,
    Qwen3,
    Gemma3,
}

#[derive(Parser, Debug)]
#[command(name = "candle-bench")]
struct Args {
    #[arg(long)]
    model_alias: String,
    #[arg(long)]
    model_dir: PathBuf,
    #[arg(long, value_enum)]
    arch: Arch,
    #[arg(long)]
    workloads_dir: PathBuf,
    #[arg(long, default_value = "5")]
    runs: usize,
    #[arg(long, default_value = "2")]
    warmup: usize,
    #[arg(long, default_value = "512")]
    max_new_tokens: usize,
    #[arg(long)]
    db: PathBuf,
    #[arg(long, default_value = "../prompt_templates/2026-04-v2.json")]
    prompts: PathBuf,
    /// Optional EOS token id; default depends on arch (best-effort guess).
    #[arg(long)]
    eos: Option<u32>,
    /// Comma-separated workload-id allow-list (e.g. ja_keigo_01,en_business_02).
    #[arg(long)]
    only: Option<String>,
}

#[derive(Deserialize)]
struct PromptFile {
    version: String,
    templates: HashMap<String, String>,
}

fn task_type_for(workload_id: &str) -> Option<&'static str> {
    for prefix in ["ja_keigo", "jp_en_mix", "en_business", "summary"] {
        if workload_id.starts_with(prefix) {
            return Some(prefix);
        }
    }
    None
}

fn extract_input_section(text: &str) -> Result<String> {
    let marker = "## INPUT";
    let start = text
        .find(marker)
        .ok_or_else(|| anyhow!("workload missing '## INPUT'"))?
        + marker.len();
    let rest = &text[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Ok(rest[..end].trim().to_string())
}

fn render_prompt(template: &str, input: &str) -> String {
    template.replacen("{input}", input, 1)
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}

fn device() -> (Device, &'static str) {
    #[cfg(feature = "metal")]
    {
        if let Ok(d) = Device::new_metal(0) {
            return (d, "metal");
        }
    }
    #[cfg(feature = "cuda")]
    {
        if let Ok(d) = Device::new_cuda(0) {
            return (d, "cuda");
        }
    }
    (Device::Cpu, "cpu")
}

enum Loaded {
    Phi3(phi3::ModelWeights),
    Llama(llama::ModelWeights),
    Qwen3(qwen3::ModelWeights),
    Gemma3(gemma3::ModelWeights),
}

impl Loaded {
    fn forward(&mut self, x: &Tensor, pos: usize) -> Result<Tensor> {
        Ok(match self {
            Loaded::Phi3(m) => m.forward(x, pos)?,
            Loaded::Llama(m) => m.forward(x, pos)?,
            Loaded::Qwen3(m) => m.forward(x, pos)?,
            Loaded::Gemma3(m) => m.forward(x, pos)?,
        })
    }
}

fn load(arch: Arch, path: &Path, dev: &Device) -> Result<Loaded> {
    let mut r = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let ct = gguf_file::Content::read(&mut r).with_context(|| "parse gguf")?;
    Ok(match arch {
        Arch::Phi3 => Loaded::Phi3(phi3::ModelWeights::from_gguf(false, ct, &mut r, dev)?),
        Arch::Llama => Loaded::Llama(llama::ModelWeights::from_gguf(ct, &mut r, dev)?),
        Arch::Qwen3 => Loaded::Qwen3(qwen3::ModelWeights::from_gguf(ct, &mut r, dev)?),
        Arch::Gemma3 => Loaded::Gemma3(gemma3::ModelWeights::from_gguf(ct, &mut r, dev)?),
    })
}

fn find_gguf(dir: &Path) -> Result<PathBuf> {
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
            hits.push(p);
        }
    }
    hits.sort();
    hits.into_iter()
        .next()
        .ok_or_else(|| anyhow!("no .gguf in {}", dir.display()))
}

fn last_logits(logits: Tensor) -> Result<Tensor> {
    match logits.dims().len() {
        2 => Ok(logits),
        3 => {
            let l = logits.dim(1)?;
            Ok(logits.i((.., l - 1, ..))?.contiguous()?)
        }
        n => Err(anyhow!("unexpected logits rank {n}")),
    }
}

struct RamSampler {
    stop: Arc<AtomicBool>,
    peak_kb: Arc<AtomicU64>,
    handle: Option<thread::JoinHandle<()>>,
}

impl RamSampler {
    fn start(interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak_kb = Arc::new(AtomicU64::new(0));
        let pid = Pid::from(std::process::id() as usize);
        let stop2 = stop.clone();
        let peak2 = peak_kb.clone();
        let handle = thread::spawn(move || {
            let mut sys = System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
            );
            while !stop2.load(Ordering::Relaxed) {
                sys.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::new().with_memory(),
                );
                let mut total: u64 = 0;
                if let Some(p) = sys.process(pid) {
                    total = p.memory(); // bytes
                }
                let kb = total / 1024;
                if kb > peak2.load(Ordering::Relaxed) {
                    peak2.store(kb, Ordering::Relaxed);
                }
                thread::sleep(interval);
            }
        });
        RamSampler {
            stop,
            peak_kb,
            handle: Some(handle),
        }
    }
    fn stop(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.peak_kb.load(Ordering::Relaxed)
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bench_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    bench_session_id    TEXT NOT NULL,
    model_alias         TEXT NOT NULL,
    model_variant       TEXT NOT NULL,
    model_repo          TEXT NOT NULL,
    model_revision      TEXT NOT NULL,
    workload_id         TEXT NOT NULL,
    ttft_ms             REAL NOT NULL,
    tokens_per_sec      REAL NOT NULL,
    peak_ram_mb         REAL NOT NULL,
    prompt_tokens       INTEGER NOT NULL,
    completion_tokens   INTEGER NOT NULL,
    completion_text     TEXT NOT NULL,
    input_hash          TEXT NOT NULL,
    output_hash         TEXT NOT NULL,
    ep_requested        TEXT NOT NULL,
    ep_actual           TEXT NOT NULL,
    ep_fallback         INTEGER NOT NULL,
    ep_fallback_reason  TEXT NOT NULL,
    run_seq             INTEGER NOT NULL,
    platform_tag        TEXT NOT NULL,
    prompt_version      TEXT NOT NULL,
    timestamp           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bench_alias_workload
    ON bench_runs(model_alias, workload_id);
CREATE INDEX IF NOT EXISTS idx_bench_join
    ON bench_runs(model_alias, input_hash, output_hash);
CREATE INDEX IF NOT EXISTS idx_bench_session
    ON bench_runs(bench_session_id);
";

fn open_db(path: &Path) -> Result<Connection> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\
         PRAGMA synchronous=NORMAL;\
         PRAGMA busy_timeout=5000;",
    )?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Avoid pulling chrono just for an audit string.
    format!("epoch:{secs}")
}

fn list_workloads(dir: &Path, only: Option<&str>) -> Result<Vec<PathBuf>> {
    let allow: Option<Vec<String>> = only.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    });
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().and_then(|x| x.to_str()) != Some("txt") {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(ref a) = allow {
            if !a.contains(&stem) {
                continue;
            }
        }
        paths.push(p);
    }
    paths.sort();
    Ok(paths)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let session_id = Uuid::new_v4().simple().to_string();
    println!("bench_session_id={session_id}");

    let prompts: PromptFile = serde_json::from_slice(&std::fs::read(&args.prompts)?)
        .with_context(|| format!("parse {}", args.prompts.display()))?;
    let prompt_version = prompts.version.clone();

    let (dev, ep_actual) = device();
    let ep_requested = ep_actual; // candle: requested == best-available; no fallback chain inside binary.
    eprintln!("device: {ep_actual}");

    let gguf_path = find_gguf(&args.model_dir)?;
    let model_variant = gguf_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let model_repo = ""; // populated from manifest in a follow-up if needed.
    let model_revision = format!(
        "local-mtime-{}",
        std::fs::metadata(&gguf_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let platform_tag = if cfg!(target_os = "macos") {
        "macos-arm64"
    } else if cfg!(target_os = "windows") {
        "windows-x64"
    } else {
        "cpu"
    };

    let workloads = list_workloads(&args.workloads_dir, args.only.as_deref())?;
    if workloads.is_empty() {
        return Err(anyhow!("no workloads matched"));
    }

    // Reuse one tokenizer; per-run model reload to guarantee KV-cache reset
    // (Codex flagged per-arch reset variability — fresh load is the safe path
    // and matches Python bench's per-run Generator construction).
    let tok_path = args.model_dir.join("tokenizer.json");
    let tokenizer = Tokenizer::from_file(&tok_path)
        .map_err(|e| anyhow!("tokenizer load failed: {e}"))?;

    let mut conn = open_db(&args.db)?;

    for wpath in &workloads {
        let workload_id = wpath
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let task = task_type_for(&workload_id)
            .ok_or_else(|| anyhow!("unknown workload prefix: {workload_id}"))?;
        let template = prompts
            .templates
            .get(task)
            .ok_or_else(|| anyhow!("no template for task {task}"))?;
        let raw = std::fs::read_to_string(wpath)?;
        let input_text = extract_input_section(&raw)?;
        let prompt = render_prompt(template, &input_text);
        let input_hash = sha256_hex(&input_text);

        eprintln!("workload {workload_id}: warmup {} + measured {}", args.warmup, args.runs);

        let total = args.warmup + args.runs;
        let tx = conn.transaction()?;
        for seq in 0..total {
            let is_warmup = seq < args.warmup;
            // Fresh load per run = guaranteed KV-cache reset and fair RAM peak.
            let mut model = load(args.arch, &gguf_path, &dev)?;
            let enc = tokenizer
                .encode(prompt.as_str(), true)
                .map_err(|e| anyhow!("encode: {e}"))?;
            let prompt_ids: Vec<u32> = enc.get_ids().to_vec();
            if prompt_ids.is_empty() {
                return Err(anyhow!("empty prompt"));
            }
            let input = Tensor::new(prompt_ids.as_slice(), &dev)?.unsqueeze(0)?;

            let sampler_proc = RamSampler::start(Duration::from_millis(50));
            let mut lp = LogitsProcessor::from_sampling(0, Sampling::ArgMax);

            let t_prefill = Instant::now();
            let logits = model.forward(&input, 0)?;
            let last = last_logits(logits)?;
            let mut next: u32 = lp.sample(&last.squeeze(0)?)?;
            let ttft = t_prefill.elapsed();

            let mut produced: Vec<u32> = Vec::with_capacity(args.max_new_tokens);
            let t_decode = Instant::now();
            for pos in prompt_ids.len()..prompt_ids.len() + args.max_new_tokens {
                produced.push(next);
                if Some(next) == args.eos {
                    break;
                }
                let step_in = Tensor::new(&[next], &dev)?.unsqueeze(0)?;
                let logits = model.forward(&step_in, pos)?;
                let last = last_logits(logits)?;
                next = lp.sample(&last.squeeze(0)?)?;
            }
            let decode_dur = t_decode.elapsed();
            let peak_ram_kb = sampler_proc.stop();
            let total_dur = ttft + decode_dur;

            if is_warmup {
                continue;
            }

            let completion = tokenizer
                .decode(&produced, true)
                .map_err(|e| anyhow!("decode: {e}"))?;
            let output_hash = sha256_hex(&completion);
            // bench_llm.py uses (prompt+decode) duration for tokens_per_sec.
            let tps = if total_dur.as_secs_f64() > 0.0 {
                produced.len() as f64 / total_dur.as_secs_f64()
            } else {
                0.0
            };

            tx.execute(
                "INSERT INTO bench_runs (
                    bench_session_id, model_alias, model_variant, model_repo, model_revision,
                    workload_id, ttft_ms, tokens_per_sec, peak_ram_mb,
                    prompt_tokens, completion_tokens, completion_text,
                    input_hash, output_hash,
                    ep_requested, ep_actual, ep_fallback, ep_fallback_reason,
                    run_seq, platform_tag, prompt_version, timestamp
                 ) VALUES (?,?,?,?,?, ?,?,?,?, ?,?,?, ?,?, ?,?,?,?, ?,?,?,?)",
                params![
                    session_id,
                    args.model_alias,
                    model_variant,
                    model_repo,
                    model_revision,
                    workload_id,
                    ttft.as_secs_f64() * 1000.0,
                    tps,
                    (peak_ram_kb as f64) / 1024.0,
                    prompt_ids.len() as i64,
                    produced.len() as i64,
                    completion,
                    input_hash,
                    output_hash,
                    ep_requested,
                    ep_actual,
                    0_i64,
                    "",
                    (seq - args.warmup) as i64,
                    platform_tag,
                    prompt_version,
                    now_iso(),
                ],
            )?;
            eprintln!(
                "  run={} ttft={}ms tok/s={:.1} ram={}MB",
                seq - args.warmup,
                ttft.as_millis(),
                tps,
                peak_ram_kb / 1024
            );
        }
        tx.commit()?;
    }

    Ok(())
}
