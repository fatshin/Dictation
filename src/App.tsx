import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type ModelInfo = {
  name: string;
  size_bytes: number;
  family: string | null;
  quantization: string | null;
};

const PROMPT_TEMPLATES: Record<string, string> = {
  ja_keigo:
    "あなたは音声口述を清書するアシスタントです。" +
    "入力は日本語の話し言葉。以下の規則で書き直してください:\n" +
    "- **必ず日本語で出力**（英訳・要約禁止）\n" +
    "- 敬体（です・ます調）の書き言葉に統一\n" +
    "- フィラー（えー、あの、まあ 等）を削除\n" +
    "- 意味を保ち、固有名詞・技術用語は原文の表記を維持\n\n" +
    "入力:\n{input}\n\n清書:\n",
  en_business:
    "You rewrite spoken dictation into polished business English.\n" +
    "- Output **English only** (do not translate to other languages).\n" +
    "- Use a formal-email register; remove fillers (um, uh, like, you know).\n" +
    "- Preserve meaning and any technical terms verbatim.\n" +
    "- Complete sentence fragments.\n\n" +
    "INPUT:\n{input}\n\nREWRITE:\n",
};

const PREFERRED_MODELS = ["gemma4:e4b", "gemma4:e2b"];

function pickDefaultModel(models: ModelInfo[]): string {
  for (const want of PREFERRED_MODELS) {
    if (models.find((m) => m.name === want)) return want;
  }
  return models[0]?.name ?? "";
}

export default function App() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [model, setModel] = useState<string>("");
  const [task, setTask] = useState<keyof typeof PROMPT_TEMPLATES>("ja_keigo");
  const [input, setInput] = useState<string>(
    "あー、田中さん、明日の打ち合わせなんだけど、30分ずらせる？10時からでお願いしたい。場所は前回と同じでいいかな。じゃ、よろしく。",
  );
  const [output, setOutput] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<ModelInfo[]>("list_models")
      .then((m) => {
        setModels(m);
        setModel(pickDefaultModel(m));
      })
      .catch((e) => setError(`list_models: ${e}`));
  }, []);

  async function rewrite() {
    if (!model) {
      setError("no model selected");
      return;
    }
    setLoading(true);
    setError(null);
    setOutput("");
    try {
      const prompt = PROMPT_TEMPLATES[task].replace("{input}", input);
      const result = await invoke<string>("rewrite_text", {
        model,
        prompt,
        maxNewTokens: 512,
      });
      setOutput(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  return (
    <main className="app">
      <header>
        <h1>Dictation</h1>
        <p className="tagline">
          Offline dictation + LLM rewrite. Phase-1a smoke shell — type below
          to test the Ollama path.
        </p>
      </header>

      <section className="row">
        <label>
          Model:&nbsp;
          <select value={model} onChange={(e) => setModel(e.target.value)}>
            {models.length === 0 && <option value="">(loading…)</option>}
            {models.map((m) => (
              <option key={m.name} value={m.name}>
                {m.name} — {(m.size_bytes / 1e9).toFixed(1)} GB
                {m.quantization ? ` (${m.quantization})` : ""}
              </option>
            ))}
          </select>
        </label>
        <label>
          Task:&nbsp;
          <select
            value={task}
            onChange={(e) =>
              setTask(e.target.value as keyof typeof PROMPT_TEMPLATES)
            }
          >
            <option value="ja_keigo">ja_keigo (敬体書き換え)</option>
            <option value="en_business">en_business (formal English)</option>
          </select>
        </label>
      </section>

      <section className="io">
        <label>Input</label>
        <textarea
          rows={6}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="raw dictation…"
        />
        <button onClick={rewrite} disabled={loading || !model}>
          {loading ? "rewriting…" : "Rewrite"}
        </button>

        <label>Output</label>
        <textarea rows={6} readOnly value={output} placeholder="(empty)" />
        {error && <div className="error">{error}</div>}
      </section>

      <footer>
        <small>
          Phase-0 gate: LLM ✅ (gemma4:e4b primary), ASR pending real-voice
          corpus. See <code>docs/ADR-002-runtime-pivot-ollama-gemma4.md</code>.
        </small>
      </footer>
    </main>
  );
}
