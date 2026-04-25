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
    "- 誤字・脱字・誤変換を正しい表記に修正（例: 「いじょう」→「以上」、「おねがいしまう」→「お願いします」）\n" +
    "- 音声認識の誤認識を文脈から推測して修正\n" +
    "- 意味を保ち、固有名詞・技術用語は原文の表記を維持\n\n" +
    "入力:\n{input}\n\n清書:\n",
  en_business:
    "You rewrite spoken dictation into polished business English.\n" +
    "- Output **English only** (do not translate to other languages).\n" +
    "- Use a formal-email register; remove fillers (um, uh, like, you know).\n" +
    "- Fix typos, misspellings, and ASR misrecognitions (infer correct words from context).\n" +
    "- Preserve meaning and any technical terms verbatim.\n" +
    "- Complete sentence fragments.\n\n" +
    "INPUT:\n{input}\n\nREWRITE:\n",
  ja_agent_task:
    "あなたは口頭指示をAIエージェント向けのタスク指示書に変換するアシスタントです。\n" +
    "入力は意味不明・断片的・口語的な音声メモです。以下の規則で整理してください:\n" +
    "- **必ず日本語で出力**\n" +
    "- フィラー・言い淀み・繰り返しを除去\n" +
    "- 誤字・脱字・誤変換・音声認識ミスを文脈から推測して修正\n" +
    "- 曖昧な指示を具体的なタスクに分解\n" +
    "- 各タスクは「何を」「どうする」が明確な1文にする\n" +
    "- 依存関係があれば順序を付ける\n" +
    "- 不明確な部分は [要確認: ...] で明示\n\n" +
    "出力フォーマット:\n" +
    "## タスク一覧\n" +
    "1. タスク内容\n" +
    "2. タスク内容\n" +
    "...\n\n" +
    "## 補足・前提条件\n" +
    "- 補足事項\n\n" +
    "入力:\n{input}\n\n整理結果:\n",
  en_agent_task:
    "You convert messy spoken notes into clear task instructions for an AI agent.\n" +
    "Input is informal, fragmented, possibly incoherent voice memo.\n" +
    "Rules:\n" +
    "- Remove fillers, false starts, repetitions\n" +
    "- Fix typos, misspellings, and ASR misrecognitions (infer correct words from context)\n" +
    "- Break down into discrete, actionable tasks\n" +
    "- Each task: one clear sentence with specific action and target\n" +
    "- Order by dependency if applicable\n" +
    "- Flag unclear parts as [NEEDS CLARIFICATION: ...]\n\n" +
    "Output format:\n" +
    "## Tasks\n" +
    "1. Task description\n" +
    "2. Task description\n" +
    "...\n\n" +
    "## Notes & Assumptions\n" +
    "- Note\n\n" +
    "INPUT:\n{input}\n\nORGANIZED TASKS:\n",
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

type SetupStatus = {
  ollama_running: boolean;
  ollama_version: string | null;
  models_installed: string[];
  models_missing: string[];
  whisper_available: boolean;
  ready: boolean;
};

type PullProgress = {
  model: string;
  status: string;
  total: number;
  completed: number;
};

export default function App() {
  const [setupDone, setSetupDone] = useState<boolean | null>(null);
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(null);
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProgress, setPullProgress] = useState<PullProgress | null>(null);

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
    invoke<SetupStatus>("check_setup").then((s) => {
      setSetupStatus(s);
      setSetupDone(s.ready);
    }).catch(() => setSetupDone(false));
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unsub: UnlistenFn | null = null;
    listen<PullProgress>("model:pull:progress", (e) => {
      if (!cancelled) setPullProgress(e.payload);
    }).then((u) => { if (cancelled) u(); else unsub = u; });
    return () => { cancelled = true; unsub?.(); };
  }, []);

  useEffect(() => {
    if (setupDone !== true) return;
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

  const [recording, setRecording] = useState(false);
  const recordingRef = useRef(false);
  const modelRef = useRef(model);
  const taskRef = useRef(task);
  const [transcript, setTranscript] = useState("");

  useEffect(() => { recordingRef.current = recording; }, [recording]);
  useEffect(() => { modelRef.current = model; }, [model]);
  useEffect(() => { taskRef.current = task; }, [task]);

  useEffect(() => {
    let cancelled = false;
    let unsub: UnlistenFn | null = null;

    listen("hotkey:dictation", async () => {
      if (cancelled) return;
      if (recordingRef.current) {
        // Stop → Transcribe → Rewrite → Paste (full pipeline)
        try {
          const text = await invoke<string>("stop_dictation");
          setRecording(false);
          setTranscript(text);
          setInput(text);

          if (text.trim()) {
            // Auto-rewrite with current task template
            setLoading(true);
            setOutput("");
            setStreaming(true);
            setError(null);
            const tmpl = PROMPT_TEMPLATES[taskRef.current] || PROMPT_TEMPLATES["ja_keigo"];
            const prompt = tmpl.replace("{input}", text);
            const currentModel = modelRef.current;
            if (currentModel) {
              const result = await invoke<string>("rewrite_streaming", {
                model: currentModel,
                prompt,
                maxNewTokens: 512,
              });
              // Auto-paste to focused app
              if (result?.trim()) {
                await new Promise((r) => setTimeout(r, 200));
                await invoke("inject_text", { text: result, mode: "clipboard" });
              }
            }
          }
        } catch (e) {
          setError(String(e));
          setRecording(false);
          setStreaming(false);
          setLoading(false);
        }
      } else {
        try {
          await invoke<string>("start_dictation");
          setRecording(true);
          setError(null);
          setTranscript("");
        } catch (e) {
          setError(String(e));
        }
      }
    }).then((u) => {
      if (cancelled) u();
      else unsub = u;
    });

    return () => {
      cancelled = true;
      unsub?.();
    };
  }, []);

  async function handleConsent() {
    await invoke("grant_consent");
    setConsented(true);
  }

  async function startRecording() {
    setError(null);
    setTranscript("");
    try {
      await invoke<string>("start_dictation");
      setRecording(true);
    } catch (e) {
      setError(String(e));
    }
  }

  async function stopRecording() {
    try {
      const text = await invoke<string>("stop_dictation");
      setRecording(false);
      setTranscript(text);
      setInput(text);
    } catch (e) {
      setError(String(e));
      setRecording(false);
    }
  }

  async function rewrite(): Promise<string | null> {
    if (!model) {
      setError("no model selected");
      return null;
    }
    setLoading(true);
    setError(null);
    setOutput("");
    setStreaming(true);
    try {
      const prompt = PROMPT_TEMPLATES[task].replace("{input}", input);
      const result = await invoke<string>("rewrite_streaming", {
        model,
        prompt,
        maxNewTokens: 512,
      });
      return result;
    } catch (e) {
      setError(String(e));
      setStreaming(false);
      setLoading(false);
      return null;
    }
  }

  async function pasteToApp() {
    if (!output) return;
    try {
      await invoke("inject_text", { text: output, mode: "clipboard" });
    } catch (e) {
      setError(String(e));
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

  async function pullMissingModel() {
    if (!setupStatus || setupStatus.models_missing.length === 0) return;
    const modelName = setupStatus.models_missing[0];
    setPulling(modelName);
    setPullProgress(null);
    try {
      await invoke("pull_model", { model: modelName });
      const updated = await invoke<SetupStatus>("check_setup");
      setSetupStatus(updated);
      setSetupDone(updated.ready);
      setPulling(null);
      setPullProgress(null);
    } catch (e) {
      setError(String(e));
      setPulling(null);
    }
  }

  if (setupDone === null) {
    return (
      <main className="app">
        <header><h1>Dictation</h1></header>
        <section className="consent"><p>Checking setup...</p></section>
      </main>
    );
  }

  if (setupDone === false && setupStatus) {
    const pct = pullProgress && pullProgress.total > 0
      ? Math.round((pullProgress.completed / pullProgress.total) * 100)
      : 0;
    return (
      <main className="app">
        <header><h1>Dictation — Setup</h1></header>
        <section className="consent">
          <h2>Initial Setup</h2>

          <div className="setup-checklist">
            <div className={setupStatus.ollama_running ? "check-ok" : "check-fail"}>
              {setupStatus.ollama_running ? "OK" : "NG"} Ollama
              {setupStatus.ollama_version && ` (v${setupStatus.ollama_version})`}
              {!setupStatus.ollama_running && (
                <p className="hint">
                  Install Ollama from <strong>ollama.com</strong> and run <code>ollama serve</code>
                </p>
              )}
            </div>

            <div className={setupStatus.models_missing.length === 0 ? "check-ok" : "check-fail"}>
              {setupStatus.models_missing.length === 0 ? "OK" : "NG"} LLM Models
              {setupStatus.models_installed.length > 0 && (
                <span className="hint"> ({setupStatus.models_installed.join(", ")})</span>
              )}
              {setupStatus.models_missing.length > 0 && (
                <div>
                  <p className="hint">Missing: {setupStatus.models_missing.join(", ")}</p>
                  {pulling ? (
                    <div className="pull-progress">
                      <div>Downloading {pulling}... {pullProgress?.status}</div>
                      {pullProgress && pullProgress.total > 0 && (
                        <div className="progress-bar">
                          <div className="progress-fill" style={{ width: `${pct}%` }} />
                          <span>{pct}%</span>
                        </div>
                      )}
                    </div>
                  ) : (
                    <button onClick={pullMissingModel} disabled={!setupStatus.ollama_running}>
                      Download {setupStatus.models_missing[0]}
                    </button>
                  )}
                </div>
              )}
            </div>

            <div className={setupStatus.whisper_available ? "check-ok" : "check-fail"}>
              {setupStatus.whisper_available ? "OK" : "NG"} Whisper ASR Model
              {!setupStatus.whisper_available && (
                <p className="hint">whisper-small.bin not found</p>
              )}
            </div>
          </div>

          {setupStatus.ollama_running && setupStatus.models_missing.length === 0 && (
            <button onClick={() => setSetupDone(true)} style={{ marginTop: "1rem" }}>
              Continue
            </button>
          )}

          {error && <div className="error">{error}</div>}
        </section>
      </main>
    );
  }

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
              <select data-model-select value={model} onChange={(e) => setModel(e.target.value)}>
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
                data-task-select
                value={task}
                onChange={(e) =>
                  setTask(e.target.value as keyof typeof PROMPT_TEMPLATES)
                }
              >
                <option value="ja_keigo">ja_keigo (敬体書き換え)</option>
                <option value="en_business">en_business (formal English)</option>
                <option value="ja_agent_task">ja_agent_task (タスク整理)</option>
                <option value="en_agent_task">en_agent_task (task organizer)</option>
              </select>
            </label>
          </section>

          <section className="io">
            <div className="row">
              <label>Input</label>
              <button
                className={recording ? "recording" : ""}
                onClick={recording ? stopRecording : startRecording}
                disabled={loading}
                style={{ marginLeft: "auto" }}
              >
                {recording ? "Stop Recording" : "Record"}
              </button>
            </div>
            {transcript && (
              <div className="transcript-info">Transcribed from voice input</div>
            )}
            <textarea
              rows={6}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="raw dictation or press Record..."
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

            <div className="row">
              <label>Output</label>
              {output && (
                <button
                  onClick={pasteToApp}
                  disabled={!output || loading}
                  style={{ marginLeft: "auto" }}
                  className="paste-btn"
                >
                  Paste to app
                </button>
              )}
            </div>
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
