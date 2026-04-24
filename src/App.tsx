import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

type ModelInfo = {
  name: string;
  size_bytes: number;
  family: string | null;
  quantization: string | null;
};

type RewriteRecord = {
  id: string;
  session_id: string;
  input_text: string;
  output_text: string;
  model: string;
  template: string;
  created_at: string;
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
const MAX_MODEL_SIZE_GB = 12;

function pickDefaultModel(models: ModelInfo[]): string {
  for (const want of PREFERRED_MODELS) {
    if (models.find((m) => m.name === want)) return want;
  }
  const suitable = models.filter((m) => m.size_bytes / 1e9 <= MAX_MODEL_SIZE_GB);
  return suitable[0]?.name ?? models[0]?.name ?? "";
}

function isOversized(m: ModelInfo): boolean {
  return m.size_bytes / 1e9 > MAX_MODEL_SIZE_GB;
}

type Tab = "rewrite" | "history";

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
  const [streaming, setStreaming] = useState(false);
  const [consented, setConsented] = useState(false);
  const [tab, setTab] = useState<Tab>("rewrite");
  const [history, setHistory] = useState<RewriteRecord[]>([]);
  const [searchQuery, setSearchQuery] = useState("");

  useEffect(() => {
    invoke<ModelInfo[]>("list_models")
      .then((m) => {
        setModels(m);
        setModel(pickDefaultModel(m));
      })
      .catch((e) => setError(`list_models: ${e}`));
  }, []);

  const unlistenRef = useRef<UnlistenFn[]>([]);

  useEffect(() => {
    let cancelled = false;

    async function setup() {
      const u1 = await listen<string>("llm:token", (e) => {
        if (!cancelled) setOutput((prev) => prev + e.payload);
      });
      const u2 = await listen<string>("llm:done", () => {
        if (!cancelled) {
          setStreaming(false);
          setLoading(false);
        }
      });
      if (cancelled) {
        u1();
        u2();
      } else {
        unlistenRef.current = [u1, u2];
      }
    }
    setup();

    return () => {
      cancelled = true;
      unlistenRef.current.forEach((u) => u());
      unlistenRef.current = [];
    };
  }, []);

  async function handleConsent() {
    await invoke("grant_consent");
    setConsented(true);
  }

  async function rewrite() {
    if (!model) {
      setError("no model selected");
      return;
    }
    setLoading(true);
    setError(null);
    setOutput("");
    setStreaming(true);
    try {
      const prompt = PROMPT_TEMPLATES[task].replace("{input}", input);
      await invoke<string>("rewrite_streaming", {
        model,
        prompt,
        maxNewTokens: 512,
      });
    } catch (e) {
      setError(String(e));
      setStreaming(false);
      setLoading(false);
    }
  }

  async function loadHistory() {
    try {
      const records = searchQuery
        ? await invoke<RewriteRecord[]>("search_history", {
            query: searchQuery,
            limit: 50,
          })
        : await invoke<RewriteRecord[]>("list_history", { limit: 50 });
      setHistory(records);
    } catch {
      setHistory([]);
    }
  }

  useEffect(() => {
    if (tab === "history") {
      loadHistory();
    }
  }, [tab]);

  if (!consented) {
    return (
      <main className="app">
        <header>
          <h1>Dictation</h1>
        </header>
        <section className="consent">
          <h2>Recording Consent</h2>
          <p>
            This application records audio from your microphone for
            transcription. By proceeding, you confirm you are authorized to
            record conversations you intend to process.
          </p>
          <p>
            You are responsible for complying with local recording-consent laws
            (one-party / two-party jurisdictions).
          </p>
          <button onClick={handleConsent}>I understand and consent</button>
        </section>
      </main>
    );
  }

  return (
    <main className="app">
      <header>
        <h1>Dictation</h1>
        <nav className="tabs">
          <button
            className={tab === "rewrite" ? "active" : ""}
            onClick={() => setTab("rewrite")}
          >
            Rewrite
          </button>
          <button
            className={tab === "history" ? "active" : ""}
            onClick={() => setTab("history")}
          >
            History
          </button>
        </nav>
      </header>

      {tab === "rewrite" && (
        <>
          <section className="row">
            <label>
              Model:&nbsp;
              <select value={model} onChange={(e) => setModel(e.target.value)}>
                {models.length === 0 && <option value="">(loading...)</option>}
                {models.map((m) => (
                  <option
                    key={m.name}
                    value={m.name}
                    style={isOversized(m) ? { color: "#999" } : undefined}
                  >
                    {m.name} — {(m.size_bytes / 1e9).toFixed(1)} GB
                    {m.quantization ? ` (${m.quantization})` : ""}
                    {isOversized(m) ? " [too large]" : ""}
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
                <option value="ja_keigo">ja_keigo</option>
                <option value="en_business">en_business</option>
              </select>
            </label>
          </section>

          <section className="io">
            <label>Input</label>
            <textarea
              rows={6}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="raw dictation..."
            />
            {models.find((m) => m.name === model && isOversized(m)) && (
              <div className="error">
                This model exceeds {MAX_MODEL_SIZE_GB} GB — may produce
                corrupted output due to VRAM limits. Use gemma4:e4b or e2b.
              </div>
            )}
            <button onClick={rewrite} disabled={loading || !model}>
              {streaming ? "streaming..." : loading ? "rewriting..." : "Rewrite"}
            </button>

            <label>Output</label>
            <textarea rows={6} readOnly value={output} placeholder="(empty)" />
            {error && <div className="error">{error}</div>}
          </section>
        </>
      )}

      {tab === "history" && (
        <section className="history">
          <div className="row">
            <input
              type="text"
              placeholder="Search history..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
            <button onClick={loadHistory}>Search</button>
          </div>
          {history.length === 0 ? (
            <p className="empty">No history yet</p>
          ) : (
            <ul className="history-list">
              {history.map((r) => (
                <li key={r.id} className="history-item">
                  <div className="history-meta">
                    {r.model} | {r.template} | {r.created_at}
                  </div>
                  <div className="history-input">{r.input_text}</div>
                  <div className="history-output">{r.output_text}</div>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
    </main>
  );
}
