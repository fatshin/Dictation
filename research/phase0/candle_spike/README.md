# candle-spike — Phase 0 Day 4

Spike to validate that `candle-transformers` can load a GGUF model
(Phi-4-mini as the smoke target) and run a single forward pass on Metal.

Scope (explicitly):

- Load GGUF weights via `candle_core::quantized::gguf_file`.
- Build the Phi-3/4 model with `quantized_phi3::ModelWeights`.
- Tokenize with `tokenizers` crate.
- Run **one** prefill forward pass. Print logits shape + wall-clock.
- No KV-cache loop. No sampling. No streaming. That's Phase 1.

This supersedes `research/phase0/rust_ort_spike/` per ADR-001.

## Build

```
cd research/phase0/candle_spike
cargo run --release --features metal -- \
    --model-dir ../downloads/phi-4-mini-gguf \
    --prompt "Hello, my name is"
```

If `phi-4-mini-gguf` is not present, download a Q4_K_M GGUF first:

```
huggingface-cli download bartowski/Phi-4-mini-instruct-GGUF \
    Phi-4-mini-instruct-Q4_K_M.gguf \
    --local-dir ../downloads/phi-4-mini-gguf
```

(Tokenizer: `microsoft/Phi-4-mini-instruct/tokenizer.json`.)

## What this proves / does not prove

- **Proves**: candle runtime loads the GGUF, Metal device initializes, one
  forward pass emits finite logits. Unblocks the Day-4 re-bench plan in
  `docs/PHASE0_POC.md §Day 4`.
- **Does not prove**: end-to-end decode speed, JP quality, KV-cache
  correctness, memory ceiling, streaming. All deferred to a full candle
  bench rewrite in a later commit.

## Known caveats (imported from the OSS survey)

- Phi-4 GGUF has fused QKV / gate+up tensors. candle's default
  `quantized_phi3.rs` may need a patched loader (sumi `src/models/phi4.rs`
  does this). If the forward pass throws a shape mismatch, that's where
  to look first.
- `tokenizers` JSON files from Microsoft's ONNX repo work — the same
  tokenizer.json we used in `rust_ort_spike`.
