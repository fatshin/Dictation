// E2E-only fake of @tauri-apps/api/core. Aliased in via vite.config.ts when
// VITE_E2E=1. The Playwright spec seeds `window.__E2E_STATE` before page load
// to control return values; everything routes through `invoke` here.

declare global {
  interface Window {
    __E2E_STATE?: any;
    __E2E_LOG?: Array<{ cmd: string; args: any }>;
  }
}

function defaults() {
  return {
    prompts: [
      {
        id: "builtin_ja_keigo",
        name: "ja_keigo",
        label: "日本語(敬体)",
        body: "rewrite this {input}{context}{dictionary}",
        language: "ja",
        is_builtin: true,
        order_idx: 0,
        created_at: "",
        updated_at: "",
      },
    ],
    dictionary: [],
    models: [
      {
        name: "qwen3.5:4b-q4_K_M",
        size_bytes: 2_600_000_000,
        family: "qwen3.5",
        quantization: "Q4_K_M",
      },
      {
        name: "gemma4:e4b",
        size_bytes: 9_600_000_000,
        family: "gemma4",
        quantization: "Q4_K_M",
      },
    ],
    asrResult: "こんにちは、本橋です。",
    rewriteResult: "こんにちは、本橋伸一です。",
    externalFocused: false,
    lastClipboard: "",
    synthPasteCount: 0,
    appSettings: {
      bypass_llm: false,
      whisper_initial_prompt: "",
    },
  };
}

function state() {
  // Tests may seed `window.__E2E_STATE` partially via addInitScript. Merge
  // any user-provided keys *over* the defaults so the rest stays valid.
  const d = defaults();
  const user = (window.__E2E_STATE ?? {}) as Partial<ReturnType<typeof defaults>>;
  window.__E2E_STATE = { ...d, ...user };
  return window.__E2E_STATE;
}

export async function invoke<T = unknown>(cmd: string, args: any = {}): Promise<T> {
  const s = state();
  if (!window.__E2E_LOG) window.__E2E_LOG = [];
  window.__E2E_LOG.push({ cmd, args });

  switch (cmd) {
    case "check_setup":
      return {
        ollama_running: true,
        ollama_version: "0.21.0",
        models_installed: s.models.map((m: any) => m.name),
        models_missing: [],
        whisper_available: true,
        ax_trusted: true,
        ready: true,
      } as any;
    case "list_models":
      return s.models;
    case "list_prompts":
      return s.prompts;
    case "list_dictionary":
      return s.dictionary;
    case "upsert_dictionary_entry": {
      const p = args.payload;
      const id = p.id || "dict_" + Math.random().toString(36).slice(2, 9);
      const existing = s.dictionary.findIndex((e: any) => e.id === id);
      const now = "2026-05-10 00:00:00";
      const entry = {
        id,
        term: p.term,
        reading: p.reading,
        aliases: p.aliases || [],
        category: p.category,
        notes: p.notes,
        created_at: existing >= 0 ? s.dictionary[existing].created_at : now,
        updated_at: now,
      };
      if (existing >= 0) s.dictionary[existing] = entry;
      else s.dictionary.push(entry);
      return entry as any;
    }
    case "delete_dictionary_entry":
      s.dictionary = s.dictionary.filter((e: any) => e.id !== args.id);
      return null as any;
    case "grant_consent":
      return null as any;
    case "start_dictation":
      return "session-1" as any;
    case "stop_dictation":
      return s.asrResult as any;
    case "get_focused_context":
      return null as any;
    case "build_rewrite_prompt":
      return ((args.template || "") as string)
        .split("{input}").join(args.input || "")
        .split("{context}").join(args.context ? `ctx:${args.context}` : "")
        .split("{dictionary}").join(args.dictionary ? `dict:${args.dictionary}` : "") as any;
    case "extract_dictionary_block":
      return "" as any;
    case "rewrite_streaming": {
      const result = s.rewriteResult;
      // Simulate streaming: dispatch events on next microtask.
      queueMicrotask(() => {
        const tokenEvt = new CustomEvent("__e2e_tauri_event__", {
          detail: { event: "llm:token", payload: result },
        });
        const doneEvt = new CustomEvent("__e2e_tauri_event__", {
          detail: { event: "llm:done", payload: result },
        });
        window.dispatchEvent(tokenEvt);
        window.dispatchEvent(doneEvt);
      });
      return result as any;
    }
    case "set_clipboard_text":
      s.lastClipboard = args.text;
      return null as any;
    case "synth_paste":
      s.synthPasteCount = (s.synthPasteCount || 0) + 1;
      return null as any;
    case "is_external_focused_now":
      return s.externalFocused as any;
    case "get_app_settings":
      return s.appSettings as any;
    case "update_app_settings":
      s.appSettings = {
        bypass_llm: !!args.settings.bypass_llm,
        whisper_initial_prompt:
          (args.settings.whisper_initial_prompt as string) ?? "",
      };
      return s.appSettings as any;
    case "get_autostart":
      return false as any;
    case "set_autostart":
      return !!args.enabled as any;
    case "generate_dictionary_candidates":
      return [] as any;
    case "list_history":
    case "search_history":
      return [] as any;
    default:
      return null as any;
  }
}
