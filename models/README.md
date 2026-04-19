# Models

Model files are **not** committed to this repo. They are downloaded on first run (or via `scripts/download_models.sh`) and verified against `MANIFEST.json`.

Default location at runtime:

- macOS: `~/Library/Application Support/Dictation/models/`
- Windows: `%LOCALAPPDATA%\Dictation\models\`

## MANIFEST.json (example)

```json
{
  "schema": "dictation-model-manifest/1",
  "models": [
    {
      "id": "phi-4-mini-instruct-onnx-int4",
      "display_name": "Phi-4 Mini Instruct (INT4)",
      "license": "MIT",
      "hf_repo": "microsoft/Phi-4-mini-instruct-onnx",
      "revision": "<pinned-commit-sha>",
      "files": [
        {"name": "model.onnx", "sha256": "<...>", "size": 2100000000},
        {"name": "model.onnx.data", "sha256": "<...>", "size": 100000000},
        {"name": "tokenizer.json", "sha256": "<...>", "size": 5000000},
        {"name": "genai_config.json", "sha256": "<...>", "size": 3000}
      ],
      "capabilities": ["text-generation"],
      "ep_support": ["coreml", "cpu", "qnn", "openvino", "directml"]
    }
  ]
}
```

The actual `MANIFEST.json` is generated at Phase 0 Day 1 once we confirm which ONNX repos exist and pin their revisions. Every hash is verified on download; a mismatch aborts the install.

## Licenses

| Model | License |
|---|---|
| Whisper (OpenAI) | MIT |
| Phi-4-mini | MIT |
| SmolLM3 | Apache 2.0 |
| Qwen3 | Apache 2.0 |
| Llama 3.2 | Llama 3.2 Community License |
| Gemma 4 | Gemma Terms of Use |

Users are prompted to accept each model's license at download time.
