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

type DictionaryCandidate = {
  term: string;
  reading: string | null;
  aliases: string[];
};

type AppSettings = {
  bypass_llm: boolean;
  whisper_initial_prompt: string;
};

// ADR-005: Whisper-only is the default. bypass_llm=true means the app
// works without Ollama / LLM models — critical for Windows where the
// DB/keystore isn't initialised yet and get_app_settings returns an error.
const DEFAULT_APP_SETTINGS: AppSettings = {
  bypass_llm: true,
  whisper_initial_prompt: "",
};

// Default candidate ranking for daily-driver use on 16GB-RAM CPU. Order
// matches research/phase0/ollama_candidates.json `ranked_priority`:
//   1. qwen3.5:4b-q4_K_M           — primary, ~2.6GB Q4_K_M
//   2. qwen3:4b-instruct-2507      — gen-over-gen reference
//   3. llm-jp-3-3.7b               — JP-specialised
//   4. gemma4:e4b / e2b            — pre-existing fallbacks
// qwen3.5:9b is bench-only (5.6GB, too slow on 16GB CPU for dictation UX).
const PREFERRED_MODELS = [
  "qwen3.5:4b-q4_K_M",
  "qwen3:4b-instruct-2507-q4_K_M",
  "hf.co/alfredplpl/llm-jp-3-3.7b-instruct-gguf:Q4_K_M",
  "gemma4:e4b",
  "gemma4:e2b",
];
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
  const [dictSuggestions, setDictSuggestions] = useState<DictionaryCandidate[]>([]);
  const lastAsrRef = useRef<string>("");
  const lastOutputRef = useRef<string>("");

  // arm-and-paste flow: after Rewrite, the result is in the clipboard and we
  // wait for the user to focus a non-Dictation app, then synth Cmd+V there.
  const pendingPasteRef = useRef(false);
  // null = idle, "armed" = waiting, {app} = success notice (auto-clears).
  const [pasteStatus, setPasteStatus] = useState<
    null | "armed" | { kind: "pasted"; app: string }
  >(null);
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
  const [appSettings, setAppSettings] =
    useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const appSettingsRef = useRef<AppSettings>(DEFAULT_APP_SETTINGS);
  useEffect(() => {
    appSettingsRef.current = appSettings;
  }, [appSettings]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        // Load app_settings BEFORE marking setup as done. Otherwise the
        // hotkey listener (registered without a setupDone gate) might fire
        // while appSettingsRef.current is still DEFAULT_APP_SETTINGS and
        // sneak the user through the LLM path when they opted into bypass.
        try {
          const settings = await invoke<AppSettings>("get_app_settings");
          if (!cancelled) {
            setAppSettings(settings);
            appSettingsRef.current = settings;
          }
        } catch {
          // DB not yet initialised (first run) — defaults are fine.
        }
        const s = await invoke<SetupStatus>("check_setup");
        if (cancelled) return;
        setSetupStatus(s);
        setSetupDone(s.ready);
      } catch (e) {
        if (cancelled) return;
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
      }
    })();
    return () => {
      cancelled = true;
    };
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
  const promptsRef = useRef<PromptTemplate[]>([]);
  const autoPasteRef = useRef(autoPaste);
  useEffect(() => { modelRef.current = model; }, [model]);
  useEffect(() => { promptIdRef.current = promptId; }, [promptId]);
  useEffect(() => { promptsRef.current = prompts; }, [prompts]);
  useEffect(() => { autoPasteRef.current = autoPaste; }, [autoPaste]);

  async function reloadPrompts() {
    try {
      const list = await invoke<PromptTemplate[]>("list_prompts");
      setPrompts(list);
      if (list.length > 0 && !list.find((p) => p.id === promptIdRef.current)) {
        // Prefer the user's last selection (by stable name) → ja_keigo → first.
        const lastName =
          (typeof localStorage !== "undefined" &&
            localStorage.getItem("dictation:lastPromptName")) ||
          "";
        const restored =
          (lastName && list.find((p) => p.name === lastName)) || null;
        const defaultPrompt =
          restored ?? list.find((p) => p.name === "ja_keigo") ?? list[0];
        setPromptId(defaultPrompt.id);
      }
    } catch (e) {
      setError(`list_prompts: ${e}`);
    }
  }

  // Persist the active template's stable name whenever the user picks one.
  useEffect(() => {
    if (!promptId) return;
    const p = prompts.find((x) => x.id === promptId);
    if (p && typeof localStorage !== "undefined") {
      localStorage.setItem("dictation:lastPromptName", p.name);
    }
  }, [promptId, prompts]);

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
    reloadAppSettings();
  }, [setupDone]);

  async function reloadAppSettings() {
    try {
      const s = await invoke<AppSettings>("get_app_settings");
      setAppSettings(s);
    } catch {
      // DB not initialised yet; keep defaults.
    }
  }

  async function saveAppSettings(next: AppSettings) {
    const saved = await invoke<AppSettings>("update_app_settings", {
      settings: next,
    });
    setAppSettings(saved);
    return saved;
  }

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

    // Resolve template via ref (handles stale closures from useEffect[]
    // listeners) and fall back to a fresh `list_prompts` fetch + ja_keigo
    // if the cache is empty (initialisation race).
    let tmpl = promptsRef.current.find((p) => p.id === selectedPromptId);
    if (!tmpl) {
      try {
        const fresh = await invoke<PromptTemplate[]>("list_prompts");
        promptsRef.current = fresh;
        setPrompts(fresh);
        tmpl =
          fresh.find((p) => p.id === selectedPromptId) ??
          fresh.find((p) => p.name === "ja_keigo") ??
          fresh[0];
        if (tmpl && tmpl.id !== selectedPromptId) {
          setPromptId(tmpl.id);
        }
      } catch (e) {
        setError(`list_prompts: ${e}`);
        return null;
      }
    }
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
        await armAutoPaste(result);
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
      const u2 = await listen<string>("llm:done", (e) => {
        if (!cancelled) {
          setStreaming(false);
          setLoading(false);
          if (typeof e.payload === "string" && e.payload.length > 0) {
            lastOutputRef.current = e.payload;
          }
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
        // Stop → Transcribe → (Rewrite or Whisper-only) → Paste
        try {
          const text = await invoke<string>("stop_dictation");
          setRecording(false);
          setTranscript(text);
          setInput(text);
          lastAsrRef.current = text;
          await handleTranscriptCompleted(text);
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
        lastAsrRef.current = text;
        await handleTranscriptCompleted(text);
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

  /// Common post-ASR step: branch on Whisper-only vs LLM rewrite. Used by
  /// the stop button, Cmd+Shift+D toggle, and fn long-press release so the
  /// bypass-LLM setting is honoured uniformly.
  async function handleTranscriptCompleted(text: string) {
    if (!text.trim()) return;
    if (appSettingsRef.current.bypass_llm) {
      setOutput(text);
      // Mirror the rewrite path so downstream consumers (e.g. dictionary
      // candidate generator) see a coherent (asr, output) pair. In bypass
      // mode the two are intentionally equal — generate_dictionary_candidates
      // will short-circuit on the equal pair.
      lastOutputRef.current = text;
      if (autoPasteRef.current) {
        await armAutoPaste(text);
      }
      return;
    }
    if (modelRef.current) {
      await runRewrite(
        text,
        modelRef.current,
        promptIdRef.current,
        autoPasteRef.current,
      );
    }
  }

  async function stopRecording() {
    try {
      const text = await invoke<string>("stop_dictation");
      setRecording(false);
      setTranscript(text);
      setInput(text);
      lastAsrRef.current = text;
      await handleTranscriptCompleted(text);
    } catch (e) {
      setError(String(e));
      setRecording(false);
    }
  }

  async function rewrite(): Promise<string | null> {
    return runRewrite(input, model, promptId, autoPaste);
  }

  function showPastedNotice(app: string) {
    setPasteStatus({ kind: "pasted", app });
    window.setTimeout(() => {
      setPasteStatus((cur) =>
        cur && typeof cur === "object" && cur.kind === "pasted" ? null : cur,
      );
    }, 2500);
  }

  async function armAutoPaste(text: string) {
    try {
      await invoke("set_clipboard_text", { text });
    } catch (e) {
      setError(`set_clipboard_text: ${e}`);
      return;
    }
    pendingPasteRef.current = true;
    setPasteStatus("armed");
    // If the user is already focused on something else (e.g. they used fn
    // long-press from inside ChatGPT and never gave Dictation focus), paste
    // immediately. Otherwise wait for the next focus:external event.
    try {
      const ext = await invoke<boolean>("is_external_focused_now");
      if (ext && pendingPasteRef.current) {
        pendingPasteRef.current = false;
        await invoke("synth_paste");
        showPastedNotice("外部アプリ");
      }
    } catch {
      // Best-effort; the focus:external listener will catch the paste later.
    }
  }

  async function pasteToApp() {
    if (!output) return;
    await armAutoPaste(output);
  }

  // Paste-on-next-focus-change watcher.
  useEffect(() => {
    let cancelled = false;
    let unsub: UnlistenFn | null = null;
    listen<string>("focus:external", async (e) => {
      if (cancelled || !pendingPasteRef.current) return;
      pendingPasteRef.current = false;
      try {
        // Tiny settle so the destination text-field has actually gained
        // first-responder status before keystrokes land.
        await new Promise((r) => setTimeout(r, 80));
        await invoke("synth_paste");
        const app = typeof e.payload === "string" && e.payload.length > 0
          ? e.payload
          : "外部アプリ";
        showPastedNotice(app);
      } catch (err) {
        setError(`synth_paste: ${err}`);
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

  async function suggestDictionaryFromLast() {
    const asr = lastAsrRef.current.trim() || input.trim();
    const out = lastOutputRef.current.trim() || output.trim();
    if (!asr || !out) {
      setError("候補生成には直近の Input/Output が必要です。録音→修正を1回実行してから試してください。");
      return;
    }
    // In bypass-LLM mode `out` is just the raw ASR, so there is no diff
    // to mine for candidates — the LLM call would be cost-only.
    if (appSettingsRef.current.bypass_llm || asr === out) {
      setError("候補生成は LLM 経由の修正結果が必要です。Whisper-only モードでは利用できません。");
      return;
    }
    if (!model) {
      setError("model 未選択");
      return;
    }
    try {
      const candidates = await invoke<DictionaryCandidate[]>(
        "generate_dictionary_candidates",
        { model, asrText: asr, rewrittenText: out },
      );
      setDictSuggestions(candidates);
      if (candidates.length === 0) {
        setError("候補は見つかりませんでした(差分が一般的な整形のみの可能性)");
      }
    } catch (e) {
      setError(`generate_dictionary_candidates: ${e}`);
    }
  }

  async function acceptSuggestion(c: DictionaryCandidate) {
    await saveDictionary({
      term: c.term,
      reading: c.reading,
      aliases: c.aliases,
    });
    setDictSuggestions((cur) => cur.filter((x) => x !== c));
  }

  function dismissSuggestion(idx: number) {
    setDictSuggestions((cur) => cur.filter((_, i) => i !== idx));
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
              rows={3}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="raw dictation or press Record..."
            />
            {models.find((m) => m.name === model && isOversized(m)) && (
              <div className="error">
                This model exceeds {MAX_MODEL_SIZE_GB} GB — may produce
                corrupted output due to VRAM limits. Use a 4B-class
                Q4_K_M model (qwen3:4b-instruct-2507-q4_K_M, gemma4:e4b).
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
              {pasteStatus === "armed" && (
                <span className="paste-hint">
                  📋 クリップボードに保存。次に開いた入力欄へ自動貼付
                </span>
              )}
              {pasteStatus && typeof pasteStatus === "object" && (
                <span className="paste-hint paste-hint-success">
                  ✅ {pasteStatus.app} に貼付しました
                </span>
              )}
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
            <textarea rows={3} readOnly value={output} placeholder="(empty)" />
            {error && <div className="error">{error}</div>}
          </section>
        </>
      )}

      {tab === "settings" && (
        <SettingsPanel
          dictionary={dictionary}
          prompts={prompts}
          suggestions={dictSuggestions}
          appSettings={appSettings}
          onSaveDict={saveDictionary}
          onDeleteDict={removeDictionary}
          onSuggestFromLast={suggestDictionaryFromLast}
          onAcceptSuggestion={acceptSuggestion}
          onDismissSuggestion={dismissSuggestion}
          onSavePrompt={savePrompt}
          onDeletePrompt={removePrompt}
          onResetPrompt={resetPromptToDefault}
          onSaveAppSettings={saveAppSettings}
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
  suggestions: DictionaryCandidate[];
  appSettings: AppSettings;
  onSaveDict: (
    p: Partial<DictionaryEntry> & { term: string },
  ) => Promise<void>;
  onDeleteDict: (id: string) => Promise<void>;
  onSuggestFromLast: () => Promise<void>;
  onAcceptSuggestion: (c: DictionaryCandidate) => Promise<void>;
  onDismissSuggestion: (idx: number) => void;
  onSavePrompt: (p: {
    id?: string;
    name: string;
    label: string;
    body: string;
    language: string;
  }) => Promise<void>;
  onDeletePrompt: (p: PromptTemplate) => Promise<void>;
  onResetPrompt: (p: PromptTemplate) => Promise<void>;
  onSaveAppSettings: (s: AppSettings) => Promise<AppSettings>;
};

function SettingsPanel(props: SettingsProps) {
  const [section, setSection] = useState<"general" | "dict" | "prompt">(
    "general",
  );
  return (
    <section className="settings">
      <nav className="tabs sub">
        <button
          className={section === "general" ? "active" : ""}
          onClick={() => setSection("general")}
        >
          一般
        </button>
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
      {section === "general" ? (
        <GeneralSettingsEditor
          settings={props.appSettings}
          onSave={props.onSaveAppSettings}
        />
      ) : section === "dict" ? (
        <DictionaryEditor
          entries={props.dictionary}
          onSave={props.onSaveDict}
          onDelete={props.onDeleteDict}
          onSuggestFromLast={props.onSuggestFromLast}
          suggestions={props.suggestions}
          onAcceptSuggestion={props.onAcceptSuggestion}
          onDismissSuggestion={props.onDismissSuggestion}
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

// Front-end cap for whisper prompt. Backend hard-caps at 700 chars; keep the
// UI limit slightly under it so save-time truncation is rare. Visible to the
// user via textarea maxLength + char counter.
const WHISPER_PROMPT_MAX = 700;

function GeneralSettingsEditor(props: {
  settings: AppSettings;
  onSave: (s: AppSettings) => Promise<AppSettings>;
}) {
  const [bypass, setBypass] = useState(props.settings.bypass_llm);
  const [prompt, setPrompt] = useState(props.settings.whisper_initial_prompt);
  const [autostart, setAutostart] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveStatus, setSaveStatus] = useState<null | "ok" | "error" | "truncated">(
    null,
  );
  const [errMsg, setErrMsg] = useState<string | null>(null);

  useEffect(() => {
    invoke<boolean>("get_autostart")
      .then(setAutostart)
      .catch(() => {});
  }, []);

  // Sync local form state when the parent's settings change (e.g. after
  // first DB load). Otherwise the form would stay on its initial defaults
  // forever. Note: a save also propagates here via the parent setting state,
  // so a server-side truncation becomes visible on the next render.
  useEffect(() => {
    setBypass(props.settings.bypass_llm);
    setPrompt(props.settings.whisper_initial_prompt);
  }, [props.settings]);

  async function save() {
    setSaving(true);
    setSaveStatus(null);
    setErrMsg(null);
    try {
      const saved = await props.onSave({
        bypass_llm: bypass,
        whisper_initial_prompt: prompt,
      });
      // Reflect the canonical value the backend stored (it may have
      // truncated NUL bytes or capped length). Without this the UI would
      // silently diverge from what's actually persisted.
      setBypass(saved.bypass_llm);
      setPrompt(saved.whisper_initial_prompt);
      const wasTruncated =
        saved.whisper_initial_prompt !== prompt && prompt.length > 0;
      setSaveStatus(wasTruncated ? "truncated" : "ok");
      window.setTimeout(() => setSaveStatus(null), 2500);
    } catch (e) {
      setSaveStatus("error");
      setErrMsg(String(e));
    } finally {
      setSaving(false);
    }
  }

  const dirty =
    bypass !== props.settings.bypass_llm ||
    prompt !== props.settings.whisper_initial_prompt;

  return (
    <div className="general-settings">
      <div className="row">
        <label>
          <input
            type="checkbox"
            checked={bypass}
            onChange={(e) => setBypass(e.target.checked)}
          />
          {" "}LLMをスキップ（Whisperの結果をそのまま貼り付け）
        </label>
      </div>
      <p className="hint">
        オンの場合、ASR後にLLMによる清書を行わず、Whisperの転写結果をそのまま使用します。
        メモリ16GB環境やLLMモデル未導入時に有効。
        <br />
        ⚠️ LLMスキップ時は<strong>辞書タブの登録は使われません</strong>。
        語彙ヒントは下の「Whisper初期プロンプト」に列挙してください。
        <br />
        ⚠️ 設定変更は<strong>次回録音から</strong>有効です（録音中の変更は現在の文字起こしには適用されません）。
      </p>

      <div className="row">
        <label>Whisper 初期プロンプト（語彙ヒント）</label>
        <span className="hint" style={{ marginLeft: "auto" }}>
          {prompt.length}/{WHISPER_PROMPT_MAX}
        </span>
      </div>
      <textarea
        rows={3}
        value={prompt}
        maxLength={WHISPER_PROMPT_MAX}
        onChange={(e) => setPrompt(e.target.value)}
        placeholder="例: Dictation, WhisperKit, Tauri, Ollama, ARI, OpenAI"
      />
      <p className="hint">
        固有名詞や専門用語を列挙すると認識精度が上がります。Whisperは末尾の約224トークンしか
        利用しないため、簡潔に。文体例（句読点スタイル等）も誘導可能。
      </p>

      <hr style={{ margin: "0.75rem 0", borderColor: "#e0e0e0" }} />

      <div className="row">
        <label>
          <input
            type="checkbox"
            checked={autostart}
            onChange={async (e) => {
              try {
                const result = await invoke<boolean>("set_autostart", {
                  enabled: e.target.checked,
                });
                setAutostart(result);
              } catch {
                // Autostart not supported or failed silently.
              }
            }}
          />
          {" "}ログイン時に自動起動
        </label>
      </div>
      <p className="hint">
        有効にすると、Mac/Windows のログイン時にバックグラウンドで起動します。
        ウィンドウを閉じてもトレイに常駐し、fn 長押し / Ctrl+Shift+D でいつでも録音可能。
      </p>

      <div className="row">
        <button onClick={save} disabled={!dirty || saving}>
          {saving ? "保存中…" : "保存"}
        </button>
        {saveStatus === "ok" && <span className="ok">✅ 保存しました</span>}
        {saveStatus === "truncated" && (
          <span className="hint">
            ✅ 保存しました（長さ・制御文字を調整しました）
          </span>
        )}
        {saveStatus === "error" && <span className="error">⚠️ {errMsg}</span>}
      </div>
    </div>
  );
}

function DictionaryEditor(props: {
  entries: DictionaryEntry[];
  onSave: (p: Partial<DictionaryEntry> & { term: string }) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onSuggestFromLast?: () => Promise<void>;
  suggestions?: DictionaryCandidate[];
  onAcceptSuggestion?: (c: DictionaryCandidate) => Promise<void>;
  onDismissSuggestion?: (idx: number) => void;
}) {
  const [editing, setEditing] = useState<Partial<DictionaryEntry> | null>(null);
  const [aliasInput, setAliasInput] = useState("");
  const [showDetails, setShowDetails] = useState(false);
  const [suggesting, setSuggesting] = useState(false);

  function startEdit(e: DictionaryEntry | null) {
    setEditing(e ?? { term: "", reading: "", aliases: [], category: "", notes: "" });
    setAliasInput("");
    setShowDetails(
      !!(e && (e.reading || (e.aliases?.length ?? 0) > 0 || e.category || e.notes)),
    );
  }

  function addAliasFromInput() {
    const v = aliasInput.trim();
    if (!v || !editing) return;
    const next = [...(editing.aliases ?? [])];
    if (!next.includes(v)) next.push(v);
    setEditing({ ...editing, aliases: next });
    setAliasInput("");
  }

  function removeAlias(i: number) {
    if (!editing) return;
    const next = [...(editing.aliases ?? [])];
    next.splice(i, 1);
    setEditing({ ...editing, aliases: next });
  }

  async function submit() {
    if (!editing?.term?.trim()) return;
    await props.onSave({
      id: editing.id,
      term: editing.term.trim(),
      reading: editing.reading?.trim() || null,
      aliases: editing.aliases ?? [],
      category: editing.category?.trim() || null,
      notes: editing.notes?.trim() || null,
    });
    setEditing(null);
  }

  async function suggest() {
    if (!props.onSuggestFromLast) return;
    setSuggesting(true);
    try {
      await props.onSuggestFromLast();
    } finally {
      setSuggesting(false);
    }
  }

  return (
    <div className="settings-section">
      <div className="row">
        <h3>辞書 ({props.entries.length})</h3>
        {props.onSuggestFromLast && (
          <button
            onClick={suggest}
            disabled={suggesting}
            style={{ marginLeft: "auto" }}
            title="直近の Output と Input の差分から辞書候補を自動生成"
          >
            {suggesting ? "生成中..." : "🤖 直近から候補生成"}
          </button>
        )}
        <button onClick={() => startEdit(null)}>+ 追加</button>
      </div>
      <p className="hint">
        音声認識の誤認識マッピングや固有名詞の表記をここに登録します。LLM 修正時に
        入力 / 文脈に出現する語のみ自動でプロンプトに差し込まれます。
      </p>

      {props.suggestions && props.suggestions.length > 0 && (
        <div className="suggestion-box">
          <div className="row">
            <strong>候補 ({props.suggestions.length})</strong>
            <span className="hint" style={{ marginLeft: "auto" }}>
              ✓ で辞書に追加、× で破棄
            </span>
          </div>
          <ul className="dict-list">
            {props.suggestions.map((c, idx) => (
              <li key={`sugg-${idx}`}>
                <div>
                  <strong>{c.term}</strong>
                  {c.reading && <span className="reading"> ({c.reading})</span>}
                  {c.aliases.length > 0 && (
                    <span className="hint"> ← {c.aliases.join(", ")}</span>
                  )}
                </div>
                <div className="row">
                  <button
                    onClick={() => props.onAcceptSuggestion?.(c)}
                  >
                    ✓ 追加
                  </button>
                  <button onClick={() => props.onDismissSuggestion?.(idx)}>
                    ✕
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}

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
              autoFocus
              value={editing.term ?? ""}
              onChange={(ev) =>
                setEditing({ ...editing, term: ev.target.value })
              }
              placeholder="本橋"
            />
          </label>

          <button
            type="button"
            className="link-btn"
            onClick={() => setShowDetails((s) => !s)}
          >
            {showDetails ? "▾ 詳細を閉じる" : "▸ 詳細(読み・誤認識・カテゴリ・メモ)"}
          </button>

          {showDetails && (
            <>
              <label>
                読み
                <input
                  value={editing.reading ?? ""}
                  onChange={(ev) =>
                    setEditing({ ...editing, reading: ev.target.value })
                  }
                  placeholder="もとはし"
                />
              </label>
              <label>
                誤認識マッピング(Enter または , で追加)
                <div className="tag-input">
                  {(editing.aliases ?? []).map((a, i) => (
                    <span className="tag" key={`${a}-${i}`}>
                      {a}
                      <button
                        type="button"
                        onClick={() => removeAlias(i)}
                        aria-label="remove"
                      >
                        ×
                      </button>
                    </span>
                  ))}
                  <input
                    value={aliasInput}
                    onChange={(ev) => setAliasInput(ev.target.value)}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter" || ev.key === ",") {
                        ev.preventDefault();
                        addAliasFromInput();
                      } else if (
                        ev.key === "Backspace" &&
                        !aliasInput &&
                        (editing.aliases?.length ?? 0) > 0
                      ) {
                        removeAlias((editing.aliases?.length ?? 1) - 1);
                      }
                    }}
                    onBlur={addAliasFromInput}
                    placeholder="元橋"
                  />
                </div>
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
            </>
          )}

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
