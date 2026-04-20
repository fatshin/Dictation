//! candle-spike: Phase-0 Day-4 spike.
//!
//! Validates that `candle-transformers` can load a GGUF file and run a
//! single forward pass on Metal (macOS) / CPU. Supersedes
//! `research/phase0/rust_ort_spike/` per ADR-001.
//!
//! Intentionally minimal: loads model, tokenizes prompt, runs one prefill,
//! prints logits shape + wall-clock. No KV-cache loop, no sampling.

use anyhow::{anyhow, Context, Result};
use candle_core::{quantized::gguf_file, Device, Tensor};
use candle_transformers::models::quantized_phi3 as phi3;
use clap::Parser;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenizers::Tokenizer;

#[derive(Parser, Debug)]
#[command(
    name = "candle-spike",
    about = "Phase-0 Day-4: load a GGUF model with candle and run one forward pass"
)]
struct Args {
    /// Directory that contains both the .gguf and tokenizer.json.
    #[arg(long)]
    model_dir: PathBuf,

    /// Explicit .gguf path override. If absent, picks the first .gguf in --model-dir.
    #[arg(long)]
    gguf: Option<PathBuf>,

    /// Explicit tokenizer.json override. If absent, expects
    /// <model-dir>/tokenizer.json.
    #[arg(long)]
    tokenizer: Option<PathBuf>,

    /// Prompt. Kept short on purpose for the spike.
    #[arg(long, default_value = "Hello, my name is")]
    prompt: String,

    /// Output JSON report.
    #[arg(long, default_value = "candle_spike_result.json")]
    output: PathBuf,
}

fn device() -> Result<Device> {
    #[cfg(feature = "metal")]
    {
        if let Ok(d) = Device::new_metal(0) {
            eprintln!("device: metal");
            return Ok(d);
        }
    }
    #[cfg(feature = "cuda")]
    {
        if let Ok(d) = Device::new_cuda(0) {
            eprintln!("device: cuda");
            return Ok(d);
        }
    }
    eprintln!("device: cpu (no accelerator feature enabled / available)");
    Ok(Device::Cpu)
}

fn find_gguf(dir: &Path) -> Result<PathBuf> {
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let p = entry?.path();
        if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
            hits.push(p);
        }
    }
    hits.sort();
    hits.into_iter()
        .next()
        .ok_or_else(|| anyhow!("no .gguf found in {}", dir.display()))
}

fn main() -> Result<()> {
    let args = Args::parse();

    let dev = device()?;

    let gguf_path = args
        .gguf
        .clone()
        .unwrap_or_else(|| find_gguf(&args.model_dir).expect("find gguf"));
    let tok_path = args
        .tokenizer
        .clone()
        .unwrap_or_else(|| args.model_dir.join("tokenizer.json"));

    // ---- load GGUF ----
    let t_load = Instant::now();
    let mut gguf_reader = std::fs::File::open(&gguf_path)
        .with_context(|| format!("open {}", gguf_path.display()))?;
    let gguf_content = gguf_file::Content::read(&mut gguf_reader)
        .with_context(|| format!("parse gguf {}", gguf_path.display()))?;
    // Phi-3/Phi-4 share the same architecture at the candle level; the
    // patched Phi-4 loader in sumi matters for quality/numerics, not for
    // a shape-only spike. If this errors with a shape mismatch, that's the
    // TODO(Phase 1): port sumi's split-tensor loader.
    let model = phi3::ModelWeights::from_gguf(gguf_content, &mut gguf_reader, &dev)
        .with_context(|| "phi3::ModelWeights::from_gguf")?;
    let load_ms = t_load.elapsed().as_millis() as u64;

    // ---- tokenize ----
    let tokenizer = Tokenizer::from_file(&tok_path)
        .map_err(|e| anyhow!("tokenizer load failed ({}): {e}", tok_path.display()))?;
    let encoded = tokenizer
        .encode(args.prompt.as_str(), true)
        .map_err(|e| anyhow!("tokenizer encode: {e}"))?;
    let ids: Vec<u32> = encoded.get_ids().to_vec();
    if ids.is_empty() {
        return Err(anyhow!("prompt tokenized to zero tokens"));
    }
    let input = Tensor::new(ids.as_slice(), &dev)?.unsqueeze(0)?;

    // ---- one forward pass ----
    let t_fwd = Instant::now();
    // `forward(x, pos)` — pos=0 for the first prefill slice.
    let logits = model.clone().forward(&input, 0)?;
    let fwd_ms = t_fwd.elapsed().as_millis() as u64;

    let logits_shape: Vec<usize> = logits.dims().to_vec();

    println!("model:          {}", gguf_path.display());
    println!("device:         {:?}", dev.location());
    println!("load_ms:        {load_ms}");
    println!("input_tokens:   {}", ids.len());
    println!("first_forward:  {fwd_ms} ms");
    println!("logits_shape:   {:?}", logits_shape);

    let report = serde_json::json!({
        "model_path": gguf_path.display().to_string(),
        "device": format!("{:?}", dev.location()),
        "load_ms": load_ms,
        "input_tokens": ids.len(),
        "first_forward_ms": fwd_ms,
        "logits_shape": logits_shape,
        "prompt": args.prompt,
        "note": "spike runs exactly one forward pass; KV-cache decode is Phase-1 work",
    });
    std::fs::write(&args.output, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", args.output.display()))?;

    Ok(())
}
