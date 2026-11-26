// The pairing flow: home, show a code, enter a code, connecting, safety code, ended.

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { CopyButton, Modal, Spinner } from "../components/Basics";
import {
  AlertIcon, ArrowLeftIcon, CheckIcon, ChevronIcon, DeviceIcon, DownloadIcon, EnterCodeIcon, LockIcon,
  RefreshIcon, ShieldIcon, ShowCodeIcon, UploadIcon, XIcon,
} from "../components/Icons";
import { TransferCard } from "../components/Cards";
import { formatClock, formatDate, formatSize, spokenCode, spokenSafetyCode } from "../lib/format";
import { useStore } from "../lib/store";
import type { SessionState } from "../lib/types";

export function Home({ onEnter, onHistory }: { onEnter(): void; onHistory(): void }) {
  const { t } = useTranslation();
  const { run, history, lang } = useStore();
  return (
    <div className="page">
      <header className="page-head">
        <h1>{t("home.title")}</h1>
        <p className="lead">{t("app.tagline")}</p>
      </header>

      <div className="choices">
        <button type="button" className="choice" onClick={() => void run((b) => b.createCode())}>
          <span className="choice-icon"><ShowCodeIcon size={28} /></span>
          <span className="choice-title">{t("home.show_title")}</span>
          <span className="choice-text">{t("home.show_text")}</span>
          <ChevronIcon className="choice-arrow" size={20} />
        </button>
        <button type="button" className="choice" onClick={onEnter}>
          <span className="choice-icon alt"><EnterCodeIcon size={28} /></span>
          <span className="choice-title">{t("home.enter_title")}</span>
          <span className="choice-text">{t("home.enter_text")}</span>
          <ChevronIcon className="choice-arrow" size={20} />
        </button>
      </div>

      <section className="panel steps" aria-labelledby="how-title">
        <h2 id="how-title">{t("home.how_title")}</h2>
        <ol>
          <li><span className="step-num">1</span>{t("home.how_1")}</li>
          <li><span className="step-num">2</span>{t("home.how_2")}</li>
          <li><span className="step-num">3</span>{t("home.how_3")}</li>
        </ol>
      </section>

      <section aria-labelledby="recent-title">
        <div className="section-head">
          <h2 id="recent-title">{t("home.recent")}</h2>
          {history.length > 0 && <button type="button" className="link-btn" onClick={onHistory}>{t("home.see_all")}</button>}
        </div>
        {history.length === 0 ? <p className="empty">{t("home.no_recent")}</p> : (
          <ul className="list">
            {history.slice(0, 4).map((entry) => (
              <li key={entry.id} className="list-row">
                <span className={`badge-icon ${entry.direction}`}>
                  {entry.direction === "send" ? <UploadIcon size={16} /> : <DownloadIcon size={16} />}
                </span>
                <div className="grow">
                  <p className="list-title"><bdi>{entry.names[0]?.replace(/\/$/, "") ?? t(`history.${entry.direction === "send" ? "sent_to" : "received_from"}`, { name: entry.peer })}</bdi></p>
                  <p className="list-sub">{t(`history.${entry.direction === "send" ? "sent_to" : "received_from"}`, { name: entry.peer })} · {formatDate(entry.at, lang)}</p>
                </div>
                <span className="muted">{formatSize(entry.bytes, lang)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function useCountdown(expiresAt: number): number {
  const [left, setLeft] = useState(() => Math.max(0, (expiresAt - Date.now()) / 1000));
  useEffect(() => {
    const tick = () => setLeft(Math.max(0, (expiresAt - Date.now()) / 1000));
    tick();
    const timer = window.setInterval(tick, 500);
    return () => window.clearInterval(timer);
  }, [expiresAt]);
  return left;
}

export function ShowCode({ session }: { session: Extract<SessionState, { state: "showing_code" | "creating_code" }> }) {
  const { t } = useTranslation();
  const { run } = useStore();
  const showing = session.state === "showing_code";
  const left = useCountdown(showing ? session.expires_at : Date.now() + 600_000);
  const cancel = () => void run(async (b) => { await b.endSession(); await b.dismissSession(); });
  const again = () => void run(async (b) => { await b.endSession(); await b.createCode(); });

  return (
    <div className="page narrow center">
      <header className="page-head">
        <h1>{t("show.title")}</h1>
      </header>
      {!showing ? (
        <div className="panel waiting"><Spinner size={22} /> <span>{t("show.preparing")}</span></div>
      ) : (
        <>
          <div className="code-panel">
            <p className="code" aria-label={spokenCode(session.code)}>
              {session.code.split("-").map((part, index, parts) => (
                <span key={index} aria-hidden="true" className={index === 0 ? "code-num" : "code-word"}>
                  {part}{index < parts.length - 1 && <span className="code-dash">-</span>}
                </span>
              ))}
            </p>
            <div className="code-meta">
              <CopyButton text={session.code} label={t("show.copy_code")} />
              <span className={`countdown ${left < 60 ? "soon" : ""}`} role="timer" aria-live="off">
                {t("show.expires", { time: formatClock(left) })}
              </span>
            </div>
            {session.qr_svg && (
              <div className="qr-wrap">
                <div className="qr" role="img" aria-label={t("show.qr_alt")} dangerouslySetInnerHTML={{ __html: session.qr_svg }} />
                <p className="muted small">{t("show.qr_hint")}</p>
              </div>
            )}
          </div>
          <p className="waiting-line" role="status"><Spinner /> {t("show.waiting")}</p>
          <p className="muted small">{t("show.once")}</p>
        </>
      )}
      <div className="actions">
        <button type="button" className="btn" onClick={cancel}>{t("common.cancel")}</button>
        {showing && <button type="button" className="btn" onClick={again}><RefreshIcon size={16} /> {t("show.new_code")}</button>}
      </div>
    </div>
  );
}

export function EnterCode({ onBack }: { onBack(): void }) {
  const { t } = useTranslation();
  const { api, notify, errorText } = useStore();
  const [value, setValue] = useState("");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [otherHost, setOtherHost] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const hintId = useId();

  // The word being typed: the last run of letters, once the number is in.
  const current = useMemo(() => {
    const match = /^\s*\d{1,4}[\s-]+(?:.*[\s-])?([a-zA-Z]+)$/.exec(value);
    return match ? match[1] : "";
  }, [value]);

  useEffect(() => {
    let live = true;
    if (current.length < 1 || !api) { setSuggestions([]); return; }
    void api.completeWord(current).then((words) => {
      if (live) setSuggestions(words.length === 1 && words[0] === current.toLowerCase() ? [] : words);
    });
    return () => { live = false; };
  }, [current, api]);

  useEffect(() => { input.current?.focus(); }, []);

  const complete = (word: string) => {
    setValue(value.slice(0, value.length - current.length) + word + "-");
    input.current?.focus();
  };

  const connect = async (allowOtherHost = false) => {
    if (!api || busy) return;
    setBusy(true);
    try {
      const check = await api.checkCode(value);
      if (check.other_host && !allowOtherHost) { setOtherHost(check.other_host); return; }
      await api.joinCode(value, allowOtherHost);
    } catch (error) {
      notify(errorText(error as { code: string; message: string }), "error");
    } finally {
      setBusy(false);
    }
  };

  const paste = async () => {
    const text = await navigator.clipboard?.readText().catch(() => "");
    if (text) setValue(text.trim().slice(0, 300));
    input.current?.focus();
  };

  return (
    <div className="page narrow center">
      <button type="button" className="back" onClick={onBack}><ArrowLeftIcon size={18} /> {t("common.back")}</button>
      <header className="page-head">
        <h1>{t("enter.title")}</h1>
      </header>
      <form className="code-form" onSubmit={(event) => { event.preventDefault(); void connect(); }}>
        <label htmlFor="code-input" className="visually-hidden">{t("enter.label")}</label>
        <div className="code-input-wrap">
          <input id="code-input" ref={input} className="code-input" value={value} placeholder={t("enter.placeholder")}
            onChange={(event) => setValue(event.target.value.slice(0, 300))}
            onKeyDown={(event) => { if (event.key === "Tab" && suggestions.length > 0 && current) { event.preventDefault(); complete(suggestions[0]); } }}
            autoComplete="off" autoCorrect="off" autoCapitalize="none" spellCheck={false} aria-describedby={hintId} />
          {"clipboard" in navigator && <button type="button" className="btn small" onClick={() => void paste()}>{t("enter.paste")}</button>}
        </div>
        <div className="suggestions" aria-label={t("enter.suggestions")} aria-live="polite">
          {suggestions.map((word) => (
            <button key={word} type="button" className="chip" onClick={() => complete(word)}>
              <strong>{word.slice(0, current.length)}</strong>{word.slice(current.length)}
            </button>
          ))}
        </div>
        <p id={hintId} className="muted small">{t("enter.hint")}</p>
        <button type="submit" className="btn primary large" disabled={busy || value.trim().length < 5}>
          {busy ? <Spinner /> : <LockIcon size={18} />} {t("enter.connect")}
        </button>
      </form>
      {otherHost && (
        <Modal title={t("enter.other_host_title")} onClose={() => setOtherHost(null)}
          actions={<>
            <button type="button" className="btn" onClick={() => setOtherHost(null)}>{t("common.cancel")}</button>
            <button type="button" className="btn primary" onClick={() => { setOtherHost(null); void connect(true); }}>{t("enter.other_host_confirm")}</button>
          </>}>
          <p>{t("enter.other_host_text", { host: otherHost })}</p>
        </Modal>
      )}
    </div>
  );
}

export function Connecting({ state }: { state: "joining" | "securing" | "connecting" }) {
  const { t } = useTranslation();
  const { run } = useStore();
  const steps: Array<{ key: string; label: string }> = [
    { key: "joining", label: t("connecting.step_find") },
    { key: "securing", label: t("connecting.step_secure") },
    { key: "connecting", label: t("connecting.step_connect") },
  ];
  const active = steps.findIndex((step) => step.key === state);
  return (
    <div className="page narrow center">
      <header className="page-head"><h1>{t("connecting.title")}</h1></header>
      <ol className="stepper" aria-live="polite">
        {steps.map((step, index) => (
          <li key={step.key} className={index < active ? "done" : index === active ? "current" : ""} aria-current={index === active ? "step" : undefined}>
            <span className="stepper-mark">{index < active ? <CheckIcon size={16} /> : index === active ? <Spinner size={16} /> : index + 1}</span>
            {step.label}
          </li>
        ))}
      </ol>
      <p className="muted small">{t("connecting.hint")}</p>
      <div className="actions">
        <button type="button" className="btn" onClick={() => void run(async (b) => { await b.endSession(); await b.dismissSession(); })}>{t("common.cancel")}</button>
      </div>
    </div>
  );
}

export function Safety({ session }: { session: Extract<SessionState, { state: "verify_safety" }> }) {
  const { t } = useTranslation();
  const { run } = useStore();
  return (
    <div className="page narrow center">
      <header className="page-head">
        <span className="hero-icon"><ShieldIcon size={30} /></span>
        <h1>{t("safety.title")}</h1>
        <p className="lead">{t("safety.intro")}</p>
      </header>
      <div className="peer-chip">
        <DeviceIcon type={session.peer.device_type} size={20} />
        <span><span className="muted">{t("safety.connected_to")}</span> <strong><bdi>{session.peer.name}</bdi></strong> <span className="muted">· {t(`device.${session.peer.device_type}`)}</span></span>
      </div>
      <p className="safety-code" aria-label={spokenSafetyCode(session.safety_code)}>
        {session.safety_code.split(" ").map((group, index) => <span key={index} aria-hidden="true">{group}</span>)}
      </p>
      {session.confirmed ? (
        <p className="waiting-line" role="status"><Spinner /> {t("safety.waiting_peer")}</p>
      ) : (
        <>
          {session.peer_confirmed && <p className="ok-line" role="status"><CheckIcon size={16} /> {t("safety.peer_confirmed")}</p>}
          <div className="actions stacked">
            <button type="button" className="btn primary large" onClick={() => void run((b) => b.confirmSafety(true))}>
              <CheckIcon size={18} /> {t("safety.match")}
            </button>
            <button type="button" className="btn danger-outline" onClick={() => void run((b) => b.confirmSafety(false))}>
              <XIcon size={16} /> {t("safety.mismatch")}
            </button>
          </div>
        </>
      )}
      <p className="muted small">{t("safety.why")}</p>
    </div>
  );
}

export function Ended({ session, wasGuest, onEnter }: {
  session: Extract<SessionState, { state: "ended" }>; wasGuest: boolean; onEnter(): void;
}) {
  const { t } = useTranslation();
  const { run, errorText, snapshot } = useStore();
  const error = session.reason === "error";
  // An error before the two devices ever met is not "a session was closed".
  const title = error && !session.peer ? "ended.title_not_connected" : `ended.title_${session.reason}`;
  const home = () => void run((b) => b.dismissSession());
  return (
    <div className="page narrow center">
      <header className="page-head">
        <span className={`hero-icon ${error ? "danger" : ""}`}>{error ? <AlertIcon size={30} /> : <CheckIcon size={30} />}</span>
        <h1>{t(title)}</h1>
        <p className="lead" role={error ? "alert" : undefined}>{error ? errorText(session.error) : t("ended.text_local")}</p>
      </header>
      <div className="actions">
        <button type="button" className="btn" onClick={home}>{t("ended.home")}</button>
        {error && !wasGuest && (
          <button type="button" className="btn primary" onClick={() => void run(async (b) => { await b.dismissSession(); await b.createCode(); })}>
            <RefreshIcon size={16} /> {t("ended.new_code")}
          </button>
        )}
        {error && wasGuest && (
          <button type="button" className="btn primary" onClick={() => { void run((b) => b.dismissSession()); onEnter(); }}>
            {t("ended.try_again")}
          </button>
        )}
      </div>
      {snapshot.transfers.length > 0 && (
        <section className="left" aria-labelledby="summary-title">
          <h2 id="summary-title">{t("ended.summary")}</h2>
          <div className="feed">{snapshot.transfers.map((transfer) => <TransferCard key={transfer.id} transfer={transfer} />)}</div>
        </section>
      )}
    </div>
  );
}
