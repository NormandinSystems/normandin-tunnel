// One place for everything the screens need: the engine's current picture,
// the settings, and the actions. Screens never call the backend directly.

import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import i18n from "../i18n";
import { backend } from "./backend";
import type { Lang } from "./format";
import { formatSize } from "./format";
import type { AppInfo, Backend, ErrorView, HistoryEntry, Settings, Snapshot, TunnelEvent } from "./types";

const EMPTY: Snapshot = { session: { state: "idle" }, offers: [], transfers: [], texts: [] };

type Action = { type: "replace"; snapshot: Snapshot } | { type: "event"; event: TunnelEvent };

function reduce(state: Snapshot, action: Action): Snapshot {
  if (action.type === "replace") return action.snapshot;
  const event = action.event;
  switch (event.type) {
    case "session":
      // A new session starts with a clean picture.
      if (event.session.state === "creating_code" || event.session.state === "joining" || event.session.state === "idle") {
        return { session: event.session, offers: [], transfers: [], texts: [] };
      }
      return { ...state, session: event.session };
    case "offer":
      return { ...state, offers: [...state.offers.filter((o) => o.id !== event.offer.id), event.offer] };
    case "offer_closed":
      return { ...state, offers: state.offers.filter((o) => o.id !== event.id) };
    case "transfer": {
      const known = state.transfers.some((t) => t.id === event.transfer.id);
      const transfers = known
        ? state.transfers.map((t) => (t.id === event.transfer.id ? event.transfer : t))
        : [...state.transfers, event.transfer];
      return { ...state, transfers };
    }
    case "text":
      return { ...state, texts: [...state.texts, event.message].slice(-500) };
    default:
      return state;
  }
}

export interface Toast {
  id: number;
  kind: "error" | "info";
  text: string;
}

interface Store {
  ready: boolean;
  snapshot: Snapshot;
  settings: Settings | null;
  info: AppInfo | null;
  history: HistoryEntry[];
  lang: Lang;
  toasts: Toast[];
  blocked: string[];
  api: Backend | null;
  saveSettings(next: Settings): Promise<boolean>;
  refreshHistory(): Promise<void>;
  /** Runs an action; failures become a toast. Returns whether it worked. */
  run(action: (api: Backend) => Promise<unknown>): Promise<boolean>;
  notify(text: string, kind?: Toast["kind"]): void;
  dismissToast(id: number): void;
  errorText(error: ErrorView | null | undefined): string;
}

const Context = createContext<Store | null>(null);

function isErrorView(value: unknown): value is ErrorView {
  return typeof value === "object" && value !== null && "code" in value && "message" in value;
}

export function StoreProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [snapshot, dispatch] = useReducer(reduce, EMPTY);
  const [api, setApi] = useState<Backend | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [blocked, setBlocked] = useState<string[]>([]);
  const nextToast = useRef(1);

  const lang: Lang = useMemo(() => {
    const wanted = settings?.language ?? "system";
    if (wanted === "en" || wanted === "fr") return wanted;
    return info?.system_language ?? "en";
  }, [settings?.language, info?.system_language]);

  useEffect(() => {
    void i18n.changeLanguage(lang);
    document.documentElement.lang = lang;
  }, [lang]);

  useEffect(() => {
    const theme = settings?.theme ?? "system";
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }, [settings?.theme]);

  const dismissToast = useCallback((id: number) => setToasts((all) => all.filter((x) => x.id !== id)), []);

  const notify = useCallback((text: string, kind: Toast["kind"] = "info") => {
    const id = nextToast.current++;
    setToasts((all) => [...all.slice(-3), { id, kind, text }]);
    window.setTimeout(() => dismissToast(id), kind === "error" ? 9000 : 4000);
  }, [dismissToast]);

  const errorText = useCallback((error: ErrorView | null | undefined) => {
    if (!error) return t("error.unknown");
    const size = error.shortfall !== undefined ? formatSize(error.shortfall, lang) : "";
    return t(`error.${error.code}`, { defaultValue: error.message || t("error.unknown"), size });
  }, [t, lang]);

  const refreshHistory = useCallback(async () => {
    if (api) setHistory(await api.getHistory().catch(() => []));
  }, [api]);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const ready = await backend();
      if (cancelled) return;
      stop = await ready.onEvent((event) => {
        if (event.type === "resync") {
          void ready.getState().then((fresh) => dispatch({ type: "replace", snapshot: fresh }));
          return;
        }
        if (event.type === "offer_blocked") setBlocked((all) => [...all, event.peer.name]);
        if (event.type === "session" && (event.session.state === "creating_code" || event.session.state === "joining")) {
          setBlocked([]);
        }
        if (event.type === "transfer" && ["done", "cancelled", "failed", "declined"].includes(event.transfer.state)) {
          void ready.getHistory().then(setHistory).catch(() => {});
        }
        dispatch({ type: "event", event });
      });
      const [fresh, loaded, about, past] = await Promise.all([
        ready.getState(), ready.getSettings(), ready.appInfo(), ready.getHistory().catch(() => []),
      ]);
      if (cancelled) return;
      dispatch({ type: "replace", snapshot: fresh });
      setSettings(loaded);
      setInfo(about);
      setHistory(past);
      setApi(ready);
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const run = useCallback(async (action: (api: Backend) => Promise<unknown>) => {
    if (!api) return false;
    try {
      await action(api);
      return true;
    } catch (error) {
      notify(isErrorView(error) ? errorText(error) : t("error.unknown"), "error");
      return false;
    }
  }, [api, notify, errorText, t]);

  const saveSettings = useCallback(async (next: Settings) => {
    if (!api) return false;
    try {
      setSettings(await api.saveSettings(next));
      return true;
    } catch (error) {
      notify(isErrorView(error) ? errorText(error) : t("error.unknown"), "error");
      return false;
    }
  }, [api, notify, errorText, t]);

  const value: Store = {
    ready: api !== null && settings !== null, snapshot, settings, info, history, lang, toasts, blocked, api,
    saveSettings, refreshHistory, run, notify, dismissToast, errorText,
  };
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useStore(): Store {
  const store = useContext(Context);
  if (!store) throw new Error("useStore must be used inside StoreProvider");
  return store;
}
