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

type PromptTemplate = {
  id: string;
  name: string;
  label: string;
  body: string;
  language: string;
  is_builtin: boolean;
  order_idx: number;
  created_at: string;
  updated_at: string;
};

type DictionaryEntry = {
  id: string;
  term: string;
  reading: string | null;
  aliases: string[];
  category: string | null;
  notes: string | null;
  created_at: string;
  updated_at: string;
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

type Tab = "rewrite" | "history" | "settings";

type SetupStatus = {
  ollama_running: boolean;
  ollama_version: string | null;
  models_installed: string[];
  models_missing: string[];
  whisper_available: boolean;
  ax_trusted: boolean;
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
  const [prompts, setPrompts] = useState<PromptTemplate[]>([]);
  const [promptId, setPromptId] = useState<string>("");
  const [dictionary, setDictionary] = useState<DictionaryEntry[]>([]);
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
  const [autoPaste, setAutoPaste] = useState(true);

  useEffect(() => {
    invoke<SetupStatus>("check_setup")
      .then((s) => {
        setSetupStatus(s);
        setSetupDone(s.ready);
      })
      .catch((e) => {
        setError(`check_setup: ${String(e)}`);
        setSetupStatus({
          ollama_running: false,
          ollama_version: null,
          models_installed: [],
          models_missing: [PREFERRED_MODELS[0]],
          whisper_available: false,
          ax_trusted: false,
          ready: false,
        });
        setSetupDone(false);
      });
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
  }, [setupDone]);

  const unlistenRef = useRef<UnlistenFn[]>([]);

  const modelRef = useRef(model);
  const promptIdRef = useRef(promptId);
  const autoPasteRef = useRef(autoPaste);
  useEffect(() => { modelRef.current = model; }, [model]);
  useEffect(() => { promptIdRef.current = promptId; }, [promptId]);
  useEffect(() => { autoPasteRef.current = autoPaste; }, [autoPaste]);

  async function reloadPrompts() {
    try {
      const list = await invoke<PromptTemplate[]>("list_prompts");
      setPrompts(list);
      if (list.length > 0 && !list.find((p) => p.id === promptIdRef.current)) {
        const defaultPrompt =
          list.find((p) => p.name === "ja_keigo") ?? list[0];
        setPromptId(defaultPrompt.id);
      }
    } catch (e) {
      setError(`list_prompts: ${e}`);
    }
  }

  async function reloadDictionary() {
    try {
      setDictionary(await invoke<DictionaryEntry[]>("list_dictionary"));
    } catch (e) {
      setError(`list_dictionary: ${e}`);
    }
  }

  useEffect(() => {
    if (setupDone !== true) return;
    reloadPrompts();
    reloadDictionary();
  }, [setupDone]);

  async function runRewrite(
    sourceText: string,
    selectedModel: string,
    selectedPromptId: string,
    shouldAutoPaste: boolean,
  ): Promise<string | null> {
    if (!selectedModel) {
      setError("no model selected");
      return null;
    }
    const tmpl = prompts.find((p) => p.id === selectedPromptId);
    if (!tmpl) {
      setError("no prompt template selected");
      return null;
    }
    setLoading(true);
    setError(null);
    setOutput("");
    setStreaming(true);
    try {
      let context: string | null = null;
      try {
        const ctx = await invoke<{ text: string; truncated: boolean } | null>(
          "get_focused_context",
        );
        if (ctx && ctx.text) context = ctx.text;
      } catch {
        // AX permission missing or focused element opaque (Electron, etc.) —
        // fall through to ASR-only.
      }

      let dictionaryBlock: string | null = null;
      try {
        const block = await invoke<string>("extract_dictionary_block", {
          input: sourceText,
          context,
        });
        if (block && block.length > 0) dictionaryBlock = block;
      } catch {
        // DB not initialised or empty dictionary — proceed without.
      }

      const prompt = await invoke<string>("build_rewrite_prompt", {
        template: tmpl.body,
        input: sourceText,
        context,
        dictionary: dictionaryBlock,
      });
      const result = await invoke<string>("rewrite_streaming", {
        model: selectedModel,
        prompt,
        maxNewTokens: 512,
      });
      if (shouldAutoPaste && result?.trim()) {
        await new Promise((r) => setTimeout(r, 300));
        await invoke("inject_text", { text: result, mode: "clipboard" });
      }
      return result;
    } catch (e) {
      setError(String(e));
      setStreaming(false);
      setLoading(false);
      return null;
    }
  }

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
  const [transcript, setTranscript] = useState("");

  useEffect(() => { recordingRef.current = recording; }, [recording]);

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
            const currentModel = modelRef.current;
            if (currentModel) {
              await runRewrite(
                text,
                currentModel,
                promptIdRef.current,
                autoPasteRef.current,
              );
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

  // fn long-press push-to-talk: hold fn 500ms → record while held, release → run pipeline
  useEffect(() => {
    let cancelled = false;
    const subs: UnlistenFn[] = [];

    listen("hotkey:press_start", async () => {
      if (cancelled || recordingRef.current) return;
      try {
        await invoke<string>("start_dictation");
        setRecording(true);
        setError(null);
        setTranscript("");
      } catch (e) {
        setError(String(e));
      }
    }).then((u) => {
      if (cancelled) u();
      else subs.push(u);
    });

    listen("hotkey:press_end", async () => {
      if (cancelled || !recordingRef.current) return;
      try {
        const text = await invoke<string>("stop_dictation");
        setRecording(false);
        setTranscript(text);
        setInput(text);
        if (text.trim() && modelRef.current) {
          await runRewrite(
            text,
            modelRef.current,
            promptIdRef.current,
            autoPasteRef.current,
          );
        }
      } catch (e) {
        setError(String(e));
        setRecording(false);
        setStreaming(false);
        setLoading(false);
      }
    }).then((u) => {
      if (cancelled) u();
      else subs.push(u);
    });

    return () => {
      cancelled = true;
      subs.forEach((u) => u());
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
    return runRewrite(input, model, promptId, autoPaste);
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

  async function saveDictionary(payload: Partial<DictionaryEntry> & { term: string }) {
    try {
      await invoke("upsert_dictionary_entry", {
        payload: {
          id: payload.id ?? null,
          term: payload.term,
          reading: payload.reading ?? null,
          aliases: payload.aliases ?? [],
          category: payload.category ?? null,
          notes: payload.notes ?? null,
        },
      });
      await reloadDictionary();
    } catch (e) {
      setError(`upsert_dictionary_entry: ${e}`);
    }
  }

  async function removeDictionary(id: string) {
    if (!confirm("この辞書エントリを削除しますか?")) return;
    try {
      await invoke("delete_dictionary_entry", { id });
      await reloadDictionary();
    } catch (e) {
      setError(`delete_dictionary_entry: ${e}`);
    }
  }

  async function savePrompt(payload: {
    id?: string;
    name: string;
    label: string;
    body: string;
    language: string;
  }) {
    if (!payload.body.includes("{input}")) {
      setError("プロンプト本文に {input} プレースホルダが必要です");
      return;
    }
    try {
      await invoke("upsert_prompt", {
        payload: {
          id: payload.id ?? null,
          name: payload.name,
          label: payload.label,
          body: payload.body,
          language: payload.language,
        },
      });
      await reloadPrompts();
    } catch (e) {
      setError(`upsert_prompt: ${e}`);
    }
  }

  async function removePrompt(p: PromptTemplate) {
    if (p.is_builtin) {
      setError("組込みテンプレートは削除できません(リセット可能)");
      return;
    }
    if (!confirm(`プロンプト「${p.label}」を削除しますか?`)) return;
    try {
      await invoke("delete_prompt", { id: p.id });
      await reloadPrompts();
    } catch (e) {
      setError(`delete_prompt: ${e}`);
    }
  }

  async function resetPromptToDefault(p: PromptTemplate) {
    if (!p.is_builtin) return;
    if (!confirm(`「${p.label}」を初期値に戻しますか?`)) return;
    try {
      await invoke("reset_prompt", { id: p.id });
      await reloadPrompts();
    } catch (e) {
      setError(`reset_prompt: ${e}`);
    }
  }

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
          <button
            className={tab === "settings" ? "active" : ""}
            onClick={() => setTab("settings")}
          >
            Settings
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
                value={promptId}
                onChange={(e) => setPromptId(e.target.value)}
              >
                {prompts.length === 0 && <option value="">(loading...)</option>}
                {prompts.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label} {p.is_builtin ? "" : "(custom)"}
                  </option>
                ))}
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
            <div className="row" style={{ alignItems: "center" }}>
              <button onClick={rewrite} disabled={loading || !model}>
                {streaming ? "streaming..." : loading ? "rewriting..." : "Rewrite"}
              </button>
              <label className="toggle-label">
                <input
                  type="checkbox"
                  checked={autoPaste}
                  onChange={(e) => setAutoPaste(e.target.checked)}
                />
                Auto-paste
              </label>
            </div>

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

      {tab === "settings" && (
        <SettingsPanel
          dictionary={dictionary}
          prompts={prompts}
          onSaveDict={saveDictionary}
          onDeleteDict={removeDictionary}
          onSavePrompt={savePrompt}
          onDeletePrompt={removePrompt}
          onResetPrompt={resetPromptToDefault}
        />
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

type SettingsProps = {
  dictionary: DictionaryEntry[];
  prompts: PromptTemplate[];
  onSaveDict: (
    p: Partial<DictionaryEntry> & { term: string },
  ) => Promise<void>;
  onDeleteDict: (id: string) => Promise<void>;
  onSavePrompt: (p: {
    id?: string;
    name: string;
    label: string;
    body: string;
    language: string;
  }) => Promise<void>;
  onDeletePrompt: (p: PromptTemplate) => Promise<void>;
  onResetPrompt: (p: PromptTemplate) => Promise<void>;
};

function SettingsPanel(props: SettingsProps) {
  const [section, setSection] = useState<"dict" | "prompt">("dict");
  return (
    <section className="settings">
      <nav className="tabs sub">
        <button
          className={section === "dict" ? "active" : ""}
          onClick={() => setSection("dict")}
        >
          辞書
        </button>
        <button
          className={section === "prompt" ? "active" : ""}
          onClick={() => setSection("prompt")}
        >
          プロンプト
        </button>
      </nav>
      {section === "dict" ? (
        <DictionaryEditor
          entries={props.dictionary}
          onSave={props.onSaveDict}
          onDelete={props.onDeleteDict}
        />
      ) : (
        <PromptEditor
          prompts={props.prompts}
          onSave={props.onSavePrompt}
          onDelete={props.onDeletePrompt}
          onReset={props.onResetPrompt}
        />
      )}
    </section>
  );
}

function DictionaryEditor(props: {
  entries: DictionaryEntry[];
  onSave: (p: Partial<DictionaryEntry> & { term: string }) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState<Partial<DictionaryEntry> | null>(null);
  const [aliasesText, setAliasesText] = useState("");

  function startEdit(e: DictionaryEntry | null) {
    setEditing(e ?? { term: "", reading: "", aliases: [], category: "", notes: "" });
    setAliasesText(e?.aliases?.join(", ") ?? "");
  }

  async function submit() {
    if (!editing?.term?.trim()) return;
    const aliases = aliasesText
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    await props.onSave({
      id: editing.id,
      term: editing.term.trim(),
      reading: editing.reading?.trim() || null,
      aliases,
      category: editing.category?.trim() || null,
      notes: editing.notes?.trim() || null,
    });
    setEditing(null);
  }

  return (
    <div className="settings-section">
      <div className="row">
        <h3>辞書 ({props.entries.length})</h3>
        <button onClick={() => startEdit(null)} style={{ marginLeft: "auto" }}>
          + 追加
        </button>
      </div>
      <p className="hint">
        音声認識の誤認識マッピングや固有名詞の表記をここに登録します。LLM 修正時に
        入力 / 文脈に出現する語のみ自動でプロンプトに差し込まれます。
      </p>

      {props.entries.length === 0 ? (
        <p className="empty">エントリなし</p>
      ) : (
        <ul className="dict-list">
          {props.entries.map((e) => (
            <li key={e.id}>
              <div>
                <strong>{e.term}</strong>
                {e.reading && <span className="reading"> ({e.reading})</span>}
                {e.category && <span className="badge">{e.category}</span>}
              </div>
              {e.aliases.length > 0 && (
                <div className="hint">aliases: {e.aliases.join(", ")}</div>
              )}
              {e.notes && <div className="hint">{e.notes}</div>}
              <div className="row">
                <button onClick={() => startEdit(e)}>編集</button>
                <button onClick={() => props.onDelete(e.id)}>削除</button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {editing && (
        <div className="edit-form">
          <label>
            表記(必須)
            <input
              value={editing.term ?? ""}
              onChange={(ev) =>
                setEditing({ ...editing, term: ev.target.value })
              }
            />
          </label>
          <label>
            読み
            <input
              value={editing.reading ?? ""}
              onChange={(ev) =>
                setEditing({ ...editing, reading: ev.target.value })
              }
            />
          </label>
          <label>
            誤認識マッピング(カンマ区切り)
            <input
              value={aliasesText}
              onChange={(ev) => setAliasesText(ev.target.value)}
              placeholder="元橋, もと橋"
            />
          </label>
          <label>
            カテゴリ
            <input
              value={editing.category ?? ""}
              onChange={(ev) =>
                setEditing({ ...editing, category: ev.target.value })
              }
              placeholder="person / company / tech / phrase"
            />
          </label>
          <label>
            メモ
            <textarea
              rows={2}
              value={editing.notes ?? ""}
              onChange={(ev) =>
                setEditing({ ...editing, notes: ev.target.value })
              }
            />
          </label>
          <div className="row">
            <button onClick={submit}>保存</button>
            <button onClick={() => setEditing(null)}>キャンセル</button>
          </div>
        </div>
      )}
    </div>
  );
}

function PromptEditor(props: {
  prompts: PromptTemplate[];
  onSave: (p: {
    id?: string;
    name: string;
    label: string;
    body: string;
    language: string;
  }) => Promise<void>;
  onDelete: (p: PromptTemplate) => Promise<void>;
  onReset: (p: PromptTemplate) => Promise<void>;
}) {
  const [editing, setEditing] = useState<{
    id?: string;
    name: string;
    label: string;
    body: string;
    language: string;
  } | null>(null);

  function startEdit(p: PromptTemplate | null) {
    setEditing(
      p
        ? {
            id: p.id,
            name: p.name,
            label: p.label,
            body: p.body,
            language: p.language,
          }
        : { name: "", label: "", body: "{input}", language: "ja" },
    );
  }

  async function submit() {
    if (!editing) return;
    if (!editing.name.trim() || !editing.label.trim()) return;
    await props.onSave(editing);
    setEditing(null);
  }

  return (
    <div className="settings-section">
      <div className="row">
        <h3>プロンプトテンプレート ({props.prompts.length})</h3>
        <button onClick={() => startEdit(null)} style={{ marginLeft: "auto" }}>
          + 追加
        </button>
      </div>
      <p className="hint">
        本文には <code>{"{input}"}</code> 必須。<code>{"{context}"}</code>{" "}
        と <code>{"{dictionary}"}</code>{" "}
        は空のとき自動で省略されます。
      </p>

      <ul className="prompt-list">
        {props.prompts.map((p) => (
          <li key={p.id}>
            <div className="row">
              <strong>{p.label}</strong>
              <span className="badge">{p.language}</span>
              {p.is_builtin && <span className="badge">builtin</span>}
              <span style={{ marginLeft: "auto" }} />
              <button onClick={() => startEdit(p)}>編集</button>
              {p.is_builtin ? (
                <button onClick={() => props.onReset(p)}>リセット</button>
              ) : (
                <button onClick={() => props.onDelete(p)}>削除</button>
              )}
            </div>
            <div className="hint" style={{ fontSize: "0.75rem" }}>
              {p.name}
            </div>
          </li>
        ))}
      </ul>

      {editing && (
        <div className="edit-form">
          <label>
            name (slug、英数字)
            <input
              value={editing.name}
              onChange={(ev) =>
                setEditing({ ...editing, name: ev.target.value })
              }
              disabled={!!editing.id}
            />
          </label>
          <label>
            label (UI表示)
            <input
              value={editing.label}
              onChange={(ev) =>
                setEditing({ ...editing, label: ev.target.value })
              }
            />
          </label>
          <label>
            language
            <select
              value={editing.language}
              onChange={(ev) =>
                setEditing({ ...editing, language: ev.target.value })
              }
            >
              <option value="ja">ja</option>
              <option value="en">en</option>
            </select>
          </label>
          <label>
            body
            <textarea
              rows={14}
              value={editing.body}
              onChange={(ev) =>
                setEditing({ ...editing, body: ev.target.value })
              }
            />
          </label>
          <div className="row">
            <button onClick={submit}>保存</button>
            <button onClick={() => setEditing(null)}>キャンセル</button>
          </div>
        </div>
      )}
    </div>
  );
}
