//! candle-spike: Phase-0 Day-4 spike. Multi-architecture variant.
//!
//! Loads a GGUF model with the right candle quantized loader (chosen by
//! --arch), runs **one** forward pass, prints logits shape + wall-clock.
//! No KV-cache loop, no sampling. Supersedes rust_ort_spike per ADR-001.

use anyhow::{anyhow, Context, Result};
use candle_core::{quantized::gguf_file, Device, Tensor};
use candle_transformers::models::{
    quantized_gemma3 as gemma3, quantized_llama as llama, quantized_phi3 as phi3,
    quantized_qwen3 as qwen3,
};
use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenizers::Tokenizer;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Arch {
    Phi3,
    Llama,
    Qwen3,
    Gemma3,
}

#[derive(Parser, Debug)]
#[command(name = "candle-spike")]
struct Args {
    #[arg(long)]
    model_dir: PathBuf,
    #[arg(long, value_enum)]
    arch: Arch,
    #[arg(long)]
    gguf: Option<PathBuf>,
    #[arg(long)]
    tokenizer: Option<PathBuf>,
    #[arg(long, default_value = "Hello, my name is")]
    prompt: String,
    #[arg(long, default_value = "candle_spike_result.json")]
    output: PathBuf,
}

fn device() -> Device {
    #[cfg(feature = "metal")]
    {
        if let Ok(d) = Device::new_metal(0) {
            eprintln!("device: metal");
            return d;
        }
    }
    #[cfg(feature = "cuda")]
    {
        if let Ok(d) = Device::new_cuda(0) {
            eprintln!("device: cuda");
            return d;
        }
    }
    eprintln!("device: cpu");
    Device::Cpu
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

fn main() -> Result<()> {
    let args = Args::parse();
    let dev = device();
    let gguf_path = args
        .gguf
        .clone()
        .unwrap_or_else(|| find_gguf(&args.model_dir).expect("find gguf"));
    let tok_path = args
        .tokenizer
        .clone()
        .unwrap_or_else(|| args.model_dir.join("tokenizer.json"));

    let t_load = Instant::now();
    let mut model = load(args.arch, &gguf_path, &dev)
        .with_context(|| format!("load {} as {:?}", gguf_path.display(), args.arch))?;
    let load_ms = t_load.elapsed().as_millis() as u64;

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

    let t_fwd = Instant::now();
    let logits = model.forward(&input, 0)?;
    let fwd_ms = t_fwd.elapsed().as_millis() as u64;
    let logits_shape: Vec<usize> = logits.dims().to_vec();

    println!("model:          {}", gguf_path.display());
    println!("arch:           {:?}", args.arch);
    println!("device:         {:?}", dev.location());
    println!("load_ms:        {load_ms}");
    println!("input_tokens:   {}", ids.len());
    println!("first_forward:  {fwd_ms} ms");
    println!("logits_shape:   {:?}", logits_shape);

    let report = serde_json::json!({
        "model_path": gguf_path.display().to_string(),
        "arch": format!("{:?}", args.arch),
        "device": format!("{:?}", dev.location()),
        "load_ms": load_ms,
        "input_tokens": ids.len(),
        "first_forward_ms": fwd_ms,
        "logits_shape": logits_shape,
        "prompt": args.prompt,
    });
    std::fs::write(&args.output, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", args.output.display()))?;
    Ok(())
}
