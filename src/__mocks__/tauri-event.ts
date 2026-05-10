// E2E-only fake of @tauri-apps/api/event. Routes through window-level
// CustomEvents dispatched by tauri-core mock so the React app can listen
// without the real Tauri IPC plumbing.

export type UnlistenFn = () => void;

export interface Event<T> {
  event: string;
  payload: T;
  id: number;
  windowLabel?: string;
}

export async function listen<T = unknown>(
  event: string,
  handler: (e: Event<T>) => void,
): Promise<UnlistenFn> {
  let id = Math.floor(Math.random() * 1e9);
  const wrapped = (e: any) => {
    if (e.detail?.event === event) {
      handler({ event, payload: e.detail.payload, id });
    }
  };
  window.addEventListener("__e2e_tauri_event__", wrapped as any);
  return () => window.removeEventListener("__e2e_tauri_event__", wrapped as any);
}

/** Helper for tests: dispatch an event manually. */
export function emit(event: string, payload: unknown) {
  window.dispatchEvent(
    new CustomEvent("__e2e_tauri_event__", { detail: { event, payload } }),
  );
}
