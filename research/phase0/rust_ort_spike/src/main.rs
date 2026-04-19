//! ort-spike: Rust spike for Dictation Phase 0.
//!
//! Goal: validate that `ort` crate v2 can load candidate ONNX models
//! (Phi-4-mini, Gemma 4 E4B, Qwen3 4B) and run at least one forward pass.
//!
//! Non-goals (explicitly out of scope for this spike):
//!   - Full KV-cache loop (Phase 1 work; requires wiring past_key_values.*
//!     / present.* tensors through a multi-step decode loop).
//!   - Multi-token generation / sampling.
//!   - EP selection (CoreML, DirectML, CUDA) — spike uses default CPU only.
//!
//! We intentionally keep this minimal so it compiles without any ONNX model
//! being present, and a single `cargo run` with `--model-dir` is enough to
//! measure first-forward latency on a real model.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::TensorRef,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "ort-spike",
    version,
    about = "Phase 0 Rust spike: load an ONNX decoder and run one forward pass via ort v2"
)]
struct Args {
    /// Path to model directory (should contain tokenizer.json and one of
    /// model.onnx / decoder_model_merged_q4.onnx / model_q4.onnx, optionally
    /// under an `onnx/` subdirectory).
    #[arg(long)]
    model_dir: PathBuf,

    /// Number of tokens to generate.
    ///
    /// NOTE: This spike currently runs exactly one forward pass regardless.
    /// The flag is kept so the CLI contract matches Phase 1's decode loop
    /// without later breaking scripts.
    #[arg(long, default_value_t = 8)]
    n_tokens: usize,

    /// Input prompt.
    #[arg(long, default_value = "Hello, my name is")]
    prompt: String,

    /// Output JSON file path.
    #[arg(long, default_value = "spike_result.json")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // ------------------------------------------------------------------
    // 1. Tokenize
    // ------------------------------------------------------------------
    let tokenizer_path = args.model_dir.join("tokenizer.json");
    let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| anyhow!("tokenizer load failed ({}): {e}", tokenizer_path.display()))?;
    let encoding = tokenizer
        .encode(args.prompt.as_str(), false)
        .map_err(|e| anyhow!("tokenizer encode failed: {e}"))?;
    let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
    let seq_len = input_ids.len();
    if seq_len == 0 {
        return Err(anyhow!("prompt tokenized to zero tokens"));
    }

    // ------------------------------------------------------------------
    // 2. Load ONNX
    // ------------------------------------------------------------------
    let model_path = find_onnx_file(&args.model_dir)?;

    // TODO(Phase 1): register execution providers explicitly here.
    //   .with_execution_providers([CoreMLExecutionProvider::default().build()])?
    //   .with_execution_providers([DirectMLExecutionProvider::default().build()])?
    // For this spike we rely on the default CPU provider to avoid needing
    // platform-specific features compiled in.
    let t_load = Instant::now();
    let session = Session::builder()
        .context("Session::builder() failed")?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .context("set optimization level failed")?
        .commit_from_file(&model_path)
        .with_context(|| format!("commit_from_file failed for {}", model_path.display()))?;
    let load_ms = t_load.elapsed().as_millis() as u64;

    // Collect model I/O metadata up front. This is cheap and gives us
    // human-readable output-name listings even if `SessionOutputs` indexing
    // is name-by-string only.
    let input_names: Vec<String> = session
        .inputs
        .iter()
        .map(|i| i.name.to_string())
        .collect();
    let output_names: Vec<String> = session
        .outputs
        .iter()
        .map(|o| o.name.to_string())
        .collect();

    // ------------------------------------------------------------------
    // 3. Run one forward pass (prefill of `seq_len` tokens).
    //
    //    NOTE: Decoder-only ONNX models exported by Optimum typically require
    //    `input_ids`, `attention_mask`, `position_ids`, and a full set of
    //    empty `past_key_values.<i>.{key,value}` tensors. We wire the first
    //    three here. If the model reports additional required inputs, the
    //    run will fail and we log the expected input names so the next
    //    iteration knows what to plumb.
    //
    //    This is intentionally a single `run()` — the spike is measuring
    //    whether ort v2 can even execute one forward, not end-to-end decode.
    // ------------------------------------------------------------------
    let attention_mask: Vec<i64> = vec![1; seq_len];
    let position_ids: Vec<i64> = (0..seq_len as i64).collect();

    let input_ids_tensor =
        TensorRef::from_array_view(([1usize, seq_len], input_ids.as_slice()))
            .context("build input_ids tensor")?;
    let attention_mask_tensor =
        TensorRef::from_array_view(([1usize, seq_len], attention_mask.as_slice()))
            .context("build attention_mask tensor")?;
    let position_ids_tensor =
        TensorRef::from_array_view(([1usize, seq_len], position_ids.as_slice()))
            .context("build position_ids tensor")?;

    let t0 = Instant::now();
    let run_result = session.run(inputs![
        "input_ids" => input_ids_tensor,
        "attention_mask" => attention_mask_tensor,
        "position_ids" => position_ids_tensor,
    ]);
    let first_forward_ms = t0.elapsed().as_millis() as u64;

    let (ran_ok, run_error, produced_output_names): (bool, Option<String>, Vec<String>) =
        match run_result {
            Ok(outputs) => {
                // ort v2 SessionOutputs is indexed by name; we can't reliably
                // enumerate via a stable public iterator across RC versions,
                // so we trust the session metadata we captured above.
                let produced: Vec<String> = output_names.clone();
                // Best-effort: peek at the first output's shape if it matches
                // a common logits name. Purely diagnostic; failures are swallowed.
                if let Some(first) = output_names.first() {
                    if let Some(v) = outputs.get(first.as_str()) {
                        if let Ok((shape, _data)) = v.try_extract_tensor::<f32>() {
                            println!("First output `{first}` shape: {:?}", shape);
                        }
                    }
                }
                (true, None, produced)
            }
            Err(e) => {
                // Common cause: the model declares required past_key_values.*
                // inputs that we didn't provide. Print the full list so the
                // next iteration knows exactly what to wire.
                eprintln!("session.run failed: {e}");
                eprintln!("Model expects inputs: {input_names:?}");
                (false, Some(e.to_string()), Vec::new())
            }
        };

    // ------------------------------------------------------------------
    // 4. Report
    // ------------------------------------------------------------------
    println!("Model loaded:      {}", model_path.display());
    println!("Model load time:   {load_ms} ms");
    println!("Input tokens:      {seq_len}");
    println!("First forward:     {first_forward_ms} ms (ran_ok={ran_ok})");
    println!("Declared inputs:   {input_names:?}");
    println!("Declared outputs:  {output_names:?}");

    let result = serde_json::json!({
        "model_path": model_path.display().to_string(),
        "model_load_ms": load_ms,
        "input_tokens": seq_len,
        "first_forward_ms": first_forward_ms,
        "ran_ok": ran_ok,
        "run_error": run_error,
        "declared_input_names": input_names,
        "declared_output_names": output_names,
        "produced_output_names": produced_output_names,
        "n_tokens_requested": args.n_tokens,
        "prompt": args.prompt,
        "note": "spike runs exactly one forward pass; full KV-cache loop is Phase 1 work",
    });
    std::fs::write(&args.output, serde_json::to_string_pretty(&result)?)
        .with_context(|| format!("writing {}", args.output.display()))?;

    // Make failure of `session.run` observable to shell callers without
    // hiding the fact that the binary itself ran to completion.
    if !ran_ok {
        std::process::exit(2);
    }
    Ok(())
}

/// Locate the ONNX weight file inside a model directory.
///
/// Checks common filenames in the root, then inside an `onnx/` subdir.
fn find_onnx_file(dir: &Path) -> Result<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "model.onnx",
        "decoder_model_merged_q4.onnx",
        "model_q4.onnx",
        "model_q4f16.onnx",
        "decoder_model_merged.onnx",
    ];

    for name in CANDIDATES {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    let onnx_subdir = dir.join("onnx");
    if onnx_subdir.is_dir() {
        for name in CANDIDATES {
            let p = onnx_subdir.join(name);
            if p.exists() {
                return Ok(p);
            }
        }
    }
    Err(anyhow!(
        "No ONNX file found in {} (checked {:?} and onnx/)",
        dir.display(),
        CANDIDATES
    ))
}
