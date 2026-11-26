// The open session: drop zone, message box and the activity feed.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Modal, Spinner } from "../components/Basics";
import { OfferCard, TextBubble, TransferCard } from "../components/Cards";
import { AlertIcon, DeviceIcon, DirectIcon, FileIcon, FolderIcon, RelayIcon, SendIcon, UploadIcon } from "../components/Icons";
import { useStore } from "../lib/store";
import type { PeerInfo, SessionState } from "../lib/types";

type Open = Extract<SessionState, { state: "active" | "reconnecting" }>;

function PathBadge({ session }: { session: Open }) {
  const { t } = useTranslation();
  if (session.state === "reconnecting") {
    return <span className="path-badge warn" role="status"><Spinner size={14} /> {t("session.reconnecting")}</span>;
  }
  if (session.path === "relay") {
    return <span className="path-badge warn" title={t("session.relay_hint")}><RelayIcon size={15} /> {t("session.relay")}</span>;
  }
  if (session.path === "direct") {
    return <span className="path-badge ok" title={t("session.direct_hint")}><DirectIcon size={15} /> {t("session.direct")}</span>;
  }
  return <span className="path-badge"><Spinner size={14} /> {t("session.unknown_path")}</span>;
}

export function Session({ session }: { session: Open }) {
  const { t } = useTranslation();
  const { snapshot, run, api, blocked } = useStore();
  const [dragging, setDragging] = useState(false);
  const [text, setText] = useState("");
  const [confirmEnd, setConfirmEnd] = useState(false);
  const feedEnd = useRef<HTMLDivElement>(null);
  const peer: PeerInfo = session.peer;
  const connected = session.state === "active";

  // Files dropped anywhere on the window are offered to the other device.
  useEffect(() => {
    if (!api) return;
    let stop: (() => void) | undefined;
    let gone = false;
    void api.onDrop((drop) => {
      if (drop.kind === "over") setDragging(true);
      else if (drop.kind === "leave") setDragging(false);
      else if (drop.kind === "drop") {
        setDragging(false);
        const paths = drop.paths;
        if (paths.length > 0) void run((b) => b.sendPaths(paths));
      }
    }).then((unlisten) => { if (gone) unlisten(); else stop = unlisten; });
    return () => { gone = true; stop?.(); };
  }, [api, run]);

  // One feed, oldest first: messages and transfers in the order they began.
  const feed = useMemo(() => {
    const items = [
      ...snapshot.texts.map((message) => ({ at: message.at, key: `text-${message.id}`, message })),
      ...snapshot.transfers.map((transfer) => ({ at: transfer.started_at, key: `transfer-${transfer.id}`, transfer })),
    ];
    return items.sort((a, b) => a.at - b.at);
  }, [snapshot.texts, snapshot.transfers]);

  const feedSize = feed.length + snapshot.offers.length;
  useEffect(() => {
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    feedEnd.current?.scrollIntoView({ behavior: reduce ? "auto" : "smooth", block: "end" });
  }, [feedSize]);

  const running = snapshot.transfers.some((transfer) => ["preparing", "offered", "active", "paused"].includes(transfer.state));
  const end = () => { if (running) setConfirmEnd(true); else void run((b) => b.endSession()); };
  const pickFiles = async () => { const paths = await api?.pickFiles(); if (paths && paths.length > 0) void run((b) => b.sendPaths(paths)); };
  const pickFolder = async () => { const path = await api?.pickFolder(); if (path) void run((b) => b.sendPaths([path])); };
  const sendText = async () => {
    const message = text.trim();
    if (!message) return;
    if (await run((b) => b.sendText(message))) setText("");
  };

  return (
    <div className="session">
      <header className="session-head">
        <div className="peer">
          <span className="peer-avatar"><DeviceIcon type={peer.device_type} size={22} /></span>
          <div>
            <h1><bdi>{peer.name}</bdi></h1>
            <p className="muted small">{t(`device.${peer.device_type}`)}</p>
          </div>
        </div>
        <PathBadge session={session} />
        <button type="button" className="btn danger-outline" onClick={end}>{t("session.end")}</button>
      </header>

      <div className="session-body">
        <section className={`dropzone ${dragging ? "over" : ""} ${connected ? "" : "disabled"}`} aria-label={t("session.drop_title")}>
          <span className="drop-icon"><UploadIcon size={24} /></span>
          <div className="drop-text">
            <p className="drop-title">{dragging ? t("session.drop_active") : t("session.drop_title")}</p>
            <p className="muted small">{t("session.drop_text")}</p>
          </div>
          <div className="actions">
            <button type="button" className="btn primary" disabled={!connected} onClick={() => void pickFiles()}><FileIcon size={16} /> {t("session.send_files")}</button>
            <button type="button" className="btn" disabled={!connected} onClick={() => void pickFolder()}><FolderIcon size={16} /> {t("session.send_folder")}</button>
          </div>
        </section>

        <section className="feed" aria-live="polite" aria-relevant="additions">
          {blocked.map((name, index) => (
            <p key={index} className="notice danger" role="alert"><AlertIcon size={16} /> {t("session.blocked", { name })}</p>
          ))}
          {feedSize === 0 && blocked.length === 0 && <p className="empty">{t("session.feed_empty")}</p>}
          {feed.map((item) =>
            "message" in item
              ? <TextBubble key={item.key} message={item.message} peerName={peer.name} />
              : <TransferCard key={item.key} transfer={item.transfer} />,
          )}
          {snapshot.offers.map((offer) => <OfferCard key={offer.id} offer={offer} />)}
          <div ref={feedEnd} />
        </section>
      </div>

      <form className="composer" onSubmit={(event) => { event.preventDefault(); void sendText(); }}>
        <label htmlFor="message" className="visually-hidden">{t("session.text_label")}</label>
        <textarea id="message" rows={1} value={text} placeholder={t("session.text_placeholder")} disabled={!connected}
          maxLength={60000} onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); void sendText(); } }} />
        <button type="submit" className="btn primary" disabled={!connected || text.trim().length === 0}>
          <SendIcon size={16} /> {t("session.send_text")}
        </button>
      </form>

      {confirmEnd && (
        <Modal title={t("session.end_confirm_title")} onClose={() => setConfirmEnd(false)}
          actions={<>
            <button type="button" className="btn" onClick={() => setConfirmEnd(false)}>{t("common.cancel")}</button>
            <button type="button" className="btn danger" onClick={() => { setConfirmEnd(false); void run((b) => b.endSession()); }}>{t("session.end")}</button>
          </>}>
          <p>{t("session.end_confirm_text")}</p>
        </Modal>
      )}
    </div>
  );
}
