// A demo engine for working on the screens in a plain browser (npm run dev).
// It is never used inside the desktop app.

import type {
  Backend, HistoryEntry, OfferView, PeerInfo, SessionState, Settings, Snapshot, TransferView, TunnelEvent,
} from "./types";

const WORDS = ["acrobat", "blizzard", "candy", "dolphin", "eclipse", "fossil", "galaxy", "harbor", "igloo", "jigsaw",
  "kiwi", "lantern", "mango", "nectar", "orbit", "pebble", "quartz", "rocket", "saffron", "tundra", "umbrella",
  "velvet", "walnut", "yogurt", "zipper"];

const QR = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 29 29" shape-rendering="crispEdges"><path fill="currentColor" d="${
  Array.from({ length: 29 * 29 }, (_, i) => {
    const [x, y] = [i % 29, Math.floor(i / 29)];
    const finder = (cx: number, cy: number) => {
      const [dx, dy] = [Math.abs(x - cx), Math.abs(y - cy)];
      return Math.max(dx, dy) <= 3 && (Math.max(dx, dy) === 3 || Math.max(dx, dy) <= 1);
    };
    const inFinderZone = (x < 8 && y < 8) || (x > 20 && y < 8) || (x < 8 && y > 20);
    const on = inFinderZone ? finder(3, 3) || finder(25, 3) || finder(3, 25) : ((x * 7 + y * 13 + x * y) % 5) < 2;
    return on ? `M${x} ${y}h1v1h-1z` : "";
  }).join("")
}"/></svg>`;

export function mockBackend(): Backend {
  const handlers = new Set<(event: TunnelEvent) => void>();
  const peer: PeerInfo = { name: "Michael's Phone", device_type: "phone", app_version: "0.1.0" };
  let settings: Settings = {
    device_name: "Michael's Laptop", language: "system", theme: "system", notifications: true, onboarded: true,
    save_dir: "/home/michael/Downloads/Tunnel", ask_where_to_save: false, conflict_rule: "keep_both",
    keep_modified_times: true, keep_executable: true, mark_downloaded: true, history: "days30",
    history_names: true, rendezvous_url: "", relay_urls: [],
    max_entries: 1_000_000, debug_logging: false,
  };
  const snapshot: Snapshot = { session: { state: "idle" }, offers: [], transfers: [], texts: [] };
  const history: HistoryEntry[] = [
    { id: 3, at: Date.now() - 3_600_000, peer: "Michael's Phone", direction: "receive", files: 214, folders: 6, bytes: 3_420_000_000, result: "done", names: ["Vacances 2026/"], folder: "/home/michael/Downloads/Tunnel" },
    { id: 2, at: Date.now() - 86_400_000, peer: "Atelier-PC", direction: "send", files: 1, folders: 0, bytes: 48_200_000, result: "done", names: ["devis-final.pdf"], folder: null },
    { id: 1, at: Date.now() - 3 * 86_400_000, peer: "NAS", direction: "send", files: 1820, folders: 44, bytes: 52_000_000_000, result: "partial", names: ["Archives/"], folder: null },
  ];
  let timers: number[] = [];
  let textId = 0;

  const emit = (event: TunnelEvent) => {
    if (event.type === "session") snapshot.session = event.session;
    if (event.type === "offer") snapshot.offers = [...snapshot.offers.filter((o) => o.id !== event.offer.id), event.offer];
    if (event.type === "offer_closed") snapshot.offers = snapshot.offers.filter((o) => o.id !== event.id);
    if (event.type === "transfer") {
      const others = snapshot.transfers.filter((t) => t.id !== event.transfer.id);
      snapshot.transfers = snapshot.transfers.some((t) => t.id === event.transfer.id)
        ? snapshot.transfers.map((t) => (t.id === event.transfer.id ? event.transfer : t))
        : [...others, event.transfer];
    }
    if (event.type === "text") snapshot.texts = [...snapshot.texts, event.message];
    handlers.forEach((handler) => handler(structuredClone(event)));
  };
  const session = (state: SessionState) => emit({ type: "session", session: state });
  const later = (ms: number, run: () => void) => timers.push(window.setTimeout(run, ms));
  const stop = () => { timers.forEach(clearTimeout); timers = []; };
  const fail = (code: string, message: string) => Promise.reject({ code, message });

  const view = (id: string, direction: "send" | "receive", label: string, files: number, bytes: number): TransferView => ({
    id, direction, state: "active", paused_by: "none", label, top_level: 1, files_total: files, folders_total: 2,
    bytes_total: bytes, bytes_done: 0, files_done: 0, files_skipped: 0, files_failed: 0, speed: 0, eta_secs: null,
    folder: direction === "receive" ? settings.save_dir : null, issues: [], issues_truncated: false, error: null,
    started_at: Date.now(),
  });

  const run = (transfer: TransferView) => {
    const speed = 38_000_000 + Math.random() * 9_000_000;
    const tick = () => {
      const current = snapshot.transfers.find((t) => t.id === transfer.id);
      if (!current || current.state === "cancelled") return;
      if (current.state === "paused") return later(300, tick);
      const done = Math.min(current.bytes_total, current.bytes_done + speed * 0.25);
      const finished = done >= current.bytes_total;
      emit({ type: "transfer", transfer: {
        ...current, bytes_done: done, speed: finished ? 0 : Math.round(speed),
        files_done: Math.round((done / current.bytes_total) * current.files_total),
        eta_secs: finished ? null : Math.ceil((current.bytes_total - done) / speed),
        state: finished ? "done" : "active",
        issues: finished && current.direction === "receive"
          ? [{ path: "Photos/CON.jpg", kind: "renamed", reason: "unsafe_name", detail: "CON.jpg" }] : [],
      } });
      if (!finished) later(250, tick);
    };
    later(250, tick);
  };

  const openSession = () => {
    session({ state: "active", peer, path: "direct" });
    later(2500, () => emit({ type: "text", message: { id: ++textId, from_peer: true, text: "Voici les photos du chalet ! https://example.com/album", at: Date.now() } }));
    later(4500, () => {
      const offer: OfferView = {
        id: "a1".repeat(16), peer, files: 214, folders: 6, bytes: 3_420_000_000,
        names: ["Vacances 2026/", "Vacances 2026/IMG_0001.jpg", "Vacances 2026/IMG_0002.jpg", "Vacances 2026/video-lac.mp4", "Vacances 2026/installer.exe"],
        warnings: [{ kind: "executables", count: 1 }, { kind: "renamed", count: 1 }],
        folder: settings.save_dir, shortfall: 0, ask_where: settings.ask_where_to_save,
      };
      emit({ type: "offer", offer });
    });
  };

  const pair = () => {
    later(900, () => session({ state: "securing" }));
    later(1700, () => session({ state: "connecting" }));
    later(2600, () => session({ state: "verify_safety", safety_code: "418 293", peer, confirmed: false, peer_confirmed: false }));
  };

  // Jump straight to a screen for design work: ?demo=code|connecting|safety|session|ended|welcome
  // plus optional &lang=fr and &theme=light.
  const params = new URLSearchParams(window.location.search);
  const demo = params.get("demo");
  if (params.get("lang") === "fr" || params.get("lang") === "en") settings.language = params.get("lang") as "fr" | "en";
  if (params.get("theme") === "light" || params.get("theme") === "dark") settings.theme = params.get("theme") as "light" | "dark";
  if (demo === "welcome") settings.onboarded = false;
  if (demo === "code") snapshot.session = { state: "showing_code", code: "4821-orbit-velvet-candy", uri: "tunnel://pair?c=4821-orbit-velvet-candy", qr_svg: QR, expires_at: Date.now() + 583_000 };
  if (demo === "connecting") snapshot.session = { state: "securing" };
  if (demo === "safety") snapshot.session = { state: "verify_safety", safety_code: "418 293", peer, confirmed: false, peer_confirmed: true };
  if (demo === "ended") snapshot.session = { state: "ended", reason: "error", error: { code: "code_expired", message: "This code expired. Make a new one." }, peer: null };
  if (demo === "session") {
    snapshot.session = { state: "active", peer, path: "direct" };
    snapshot.texts = [
      { id: ++textId, from_peer: true, text: "Voici les photos du chalet ! https://example.com/album", at: Date.now() - 240_000 },
      { id: ++textId, from_peer: false, text: "Parfait, merci. J'envoie le devis.", at: Date.now() - 200_000 },
    ];
    snapshot.transfers = [
      { ...view("b2".repeat(16), "send", "devis-final.pdf", 1, 48_200_000), state: "done", bytes_done: 48_200_000, files_done: 1, started_at: Date.now() - 180_000 },
      { ...view("c3".repeat(16), "receive", "Vacances 2026", 214, 3_420_000_000), bytes_done: 1_573_000_000, files_done: 98, speed: 41_300_000, eta_secs: 45, started_at: Date.now() - 60_000 },
    ];
    snapshot.offers = [{
      id: "a1".repeat(16), peer, files: 3, folders: 0, bytes: 12_400_000,
      names: ["facture.pdf.exe", "notes.txt", "plan.dwg"],
      warnings: [{ kind: "double_extensions", count: 1 }, { kind: "executables", count: 1 }],
      folder: settings.save_dir, shortfall: 0, ask_where: false,
    }];
  }

  return {
    getState: async () => structuredClone(snapshot),
    getSettings: async () => ({ ...settings }),
    saveSettings: async (next) => (settings = { ...next }),
    getHistory: async () => [...history],
    clearHistory: async () => { history.length = 0; },
    appInfo: async () => ({ version: "0.1.0", platform: "demo", data_dir: "~/.local/share/NormandinSystems/Tunnel", default_save_dir: "/home/michael/Downloads/Tunnel", system_language: navigator.language.startsWith("fr") ? "fr" : "en" }),
    completeWord: async (prefix) => WORDS.filter((w) => w.startsWith(prefix.toLowerCase())).slice(0, 6),
    checkCode: async (input) => (/^\d{1,4}([-\s]+[a-z-]{3,}){3}$/i.test(input.trim())
      ? { code: input.trim().toLowerCase().replace(/\s+/g, "-"), other_host: null }
      : fail("code_invalid", "That doesn't look like a code.")),
    createCode: async () => {
      stop();
      snapshot.offers = []; snapshot.transfers = []; snapshot.texts = [];
      session({ state: "creating_code" });
      later(500, () => session({ state: "showing_code", code: "4821-orbit-velvet-candy", uri: "tunnel://pair?c=4821-orbit-velvet-candy&s=rendezvous.normandinsystems.com", qr_svg: QR, expires_at: Date.now() + 600_000 }));
      later(5000, pair);
    },
    joinCode: async () => {
      stop();
      snapshot.offers = []; snapshot.transfers = []; snapshot.texts = [];
      session({ state: "joining" });
      pair();
    },
    confirmSafety: async (matches) => {
      if (!matches) return session({ state: "ended", reason: "error", error: { code: "safety_mismatch", message: "The safety codes didn't match, so the session was ended." }, peer });
      session({ state: "verify_safety", safety_code: "418 293", peer, confirmed: true, peer_confirmed: false });
      later(1200, openSession);
    },
    sendPaths: async (paths) => {
      const id = Math.random().toString(16).slice(2).padEnd(32, "0").slice(0, 32);
      const label = paths[0]?.split(/[\\/]/).pop() ?? "files";
      const transfer = { ...view(id, "send", label, 12, 840_000_000), state: "offered" as const };
      emit({ type: "transfer", transfer });
      later(1500, () => { emit({ type: "transfer", transfer: { ...transfer, state: "active" } }); run(transfer); });
      return id;
    },
    sendText: async (text) => emit({ type: "text", message: { id: ++textId, from_peer: false, text, at: Date.now() } }),
    respondOffer: async (id, accept, folder) => {
      const offer = snapshot.offers.find((o) => o.id === id);
      emit({ type: "offer_closed", id });
      if (!offer || !accept) return;
      const transfer = { ...view(id, "receive", "Vacances 2026", offer.files, offer.bytes), folder: folder ?? offer.folder };
      emit({ type: "transfer", transfer });
      run(transfer);
    },
    pauseTransfer: async (id) => { const t = snapshot.transfers.find((x) => x.id === id); if (t) emit({ type: "transfer", transfer: { ...t, state: "paused", paused_by: "local", speed: 0, eta_secs: null } }); },
    resumeTransfer: async (id) => { const t = snapshot.transfers.find((x) => x.id === id); if (t) emit({ type: "transfer", transfer: { ...t, state: "active", paused_by: "none" } }); },
    cancelTransfer: async (id) => { const t = snapshot.transfers.find((x) => x.id === id); if (t) emit({ type: "transfer", transfer: { ...t, state: "cancelled", speed: 0, eta_secs: null } }); },
    endSession: async () => { stop(); session({ state: "ended", reason: "local", error: null, peer: snapshot.session.state === "active" ? peer : null }); },
    dismissSession: async () => { stop(); snapshot.offers = []; snapshot.transfers = []; snapshot.texts = []; session({ state: "idle" }); },
    openFolder: async () => {},
    pickFiles: async () => ["/home/michael/Documents/Projet/"],
    pickFolder: async () => "/home/michael/Documents",
    copyText: async (text) => { await navigator.clipboard?.writeText(text).catch(() => {}); },
    onEvent: async (handler) => { handlers.add(handler); return () => handlers.delete(handler); },
    onDrop: async () => () => {},
  };
}
