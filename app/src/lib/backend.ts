// The one place the UI talks to the Rust side. Inside the desktop app this is
// the Tauri command allowlist; in a plain browser (npm run dev) it is a demo
// backend so the screens can be worked on without building the app.

import type { Backend, TunnelEvent } from "./types";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const clipboard = await import("@tauri-apps/plugin-clipboard-manager");

  return {
    getState: () => invoke("get_state"),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    getHistory: () => invoke("get_history"),
    clearHistory: () => invoke("clear_history"),
    appInfo: () => invoke("app_info"),
    completeWord: (prefix) => invoke("complete_word", { prefix }),
    checkCode: (input) => invoke("check_code", { input }),
    createCode: () => invoke("create_code"),
    joinCode: (input, allowOtherHost) => invoke("join_code", { input, allowOtherHost }),
    confirmSafety: (matches) => invoke("confirm_safety", { matches }),
    sendPaths: (paths) => invoke("send_paths", { paths }),
    sendText: (text) => invoke("send_text", { text }),
    respondOffer: (id, accept, folder) => invoke("respond_offer", { id, accept, folder: folder ?? null }),
    pauseTransfer: (id) => invoke("pause_transfer", { id }),
    resumeTransfer: (id) => invoke("resume_transfer", { id }),
    cancelTransfer: (id) => invoke("cancel_transfer", { id }),
    endSession: () => invoke("end_session"),
    dismissSession: () => invoke("dismiss_session"),
    openFolder: (path) => invoke("open_folder", { path }),
    pickFiles: async () => {
      const picked = await dialog.open({ multiple: true, directory: false });
      if (picked === null) return [];
      return Array.isArray(picked) ? picked : [picked];
    },
    pickFolder: async (title) => {
      const picked = await dialog.open({ multiple: false, directory: true, title });
      return typeof picked === "string" ? picked : null;
    },
    copyText: (text) => clipboard.writeText(text),
    onEvent: (handler) => listen<TunnelEvent>("tunnel-event", (event) => handler(event.payload)),
    onDrop: (handler) =>
      getCurrentWebview().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "drop") handler({ kind: "drop", paths: payload.paths });
        else if (payload.type === "leave") handler({ kind: "leave" });
        else handler({ kind: "over" });
      }),
  };
}

let cached: Promise<Backend> | undefined;

export function backend(): Promise<Backend> {
  cached ??= inTauri ? tauriBackend() : import("./mock").then((m) => m.mockBackend());
  return cached;
}

export const isDesktopApp = inTauri;
