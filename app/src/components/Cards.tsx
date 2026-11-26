// The cards of the session feed: an incoming offer, a transfer, a text message.

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { formatCount, formatDuration, formatSize, formatTime, splitLinks } from "../lib/format";
import { useStore } from "../lib/store";
import type { Issue, OfferView, OfferWarning, TextView, TransferView } from "../lib/types";
import { CopyButton, Modal, ProgressBar } from "./Basics";
import {
  AlertIcon, CheckIcon, DownloadIcon, FileIcon, FolderIcon, LinkIcon, PauseIcon, PlayIcon, UploadIcon, XIcon,
} from "./Icons";

function useCounts() {
  const { t } = useTranslation();
  const { lang } = useStore();
  return (files: number, folders: number) => {
    const parts = [t("offer.files", { count: files, formatted: formatCount(files, lang) })];
    if (folders > 0) parts.push(t("offer.folders", { count: folders, formatted: formatCount(folders, lang) }));
    return parts.join(" · ");
  };
}

function warningText(t: ReturnType<typeof useTranslation>["t"], warning: OfferWarning): string {
  if (warning.kind === "low_space") return t("offer.warn_low_space");
  return t(`offer.warn_${warning.kind}`, { count: warning.count });
}

export function OfferCard({ offer }: { offer: OfferView }) {
  const { t } = useTranslation();
  const { lang, run, api, errorText, notify } = useStore();
  const counts = useCounts();
  const [folder, setFolder] = useState(offer.folder);
  const [shortfall, setShortfall] = useState(offer.shortfall);
  const [showNames, setShowNames] = useState(false);
  const [busy, setBusy] = useState(false);
  const total = offer.files + offer.folders;
  const serious = offer.warnings.some((w) => w.kind === "double_extensions" || w.kind === "bidi_names");

  const chooseFolder = async () => {
    const picked = await api?.pickFolder(t("offer.save_to"));
    if (picked) { setFolder(picked); setShortfall(0); }
    return picked ?? null;
  };

  const accept = async () => {
    if (!api) return;
    let target = folder;
    if (offer.ask_where) {
      const picked = await chooseFolder();
      if (!picked) return;
      target = picked;
    }
    setBusy(true);
    try {
      await api.respondOffer(offer.id, true, target);
    } catch (error) {
      const view = error as { code?: string; message?: string; shortfall?: number };
      if (view.code === "no_space" && view.shortfall) setShortfall(view.shortfall);
      notify(errorText(view.code ? { code: view.code, message: view.message ?? "", shortfall: view.shortfall } : null), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className={`card offer ${serious ? "serious" : ""}`} aria-label={t("offer.title", { name: offer.peer.name })}>
      <header className="card-head">
        <span className="badge-icon receive"><DownloadIcon size={18} /></span>
        <div>
          <h3>{t("offer.title", { name: offer.peer.name })}</h3>
          <p className="card-sub">{counts(offer.files, offer.folders)} · <strong>{formatSize(offer.bytes, lang)}</strong></p>
        </div>
      </header>

      <ul className={`names ${showNames ? "open" : ""}`}>
        {offer.names.slice(0, showNames ? 20 : 3).map((name) => (
          <li key={name}>
            {name.endsWith("/") ? <FolderIcon size={15} /> : <FileIcon size={15} />}
            <bdi>{name.replace(/\/$/, "")}</bdi>
          </li>
        ))}
        {total > (showNames ? Math.min(20, offer.names.length) : 3) && (
          <li className="more">
            {showNames && total > offer.names.length && <span>{t("common.and_more", { count: total - offer.names.length })}</span>}
            {offer.names.length > 3 && (
              <button type="button" className="link-btn" onClick={() => setShowNames(!showNames)}>
                {showNames ? t("offer.hide_names") : t("offer.show_names")}
              </button>
            )}
          </li>
        )}
      </ul>

      {offer.warnings.length > 0 && (
        <ul className="warnings">
          {offer.warnings.map((warning) => (
            <li key={warning.kind} className={warning.kind === "double_extensions" || warning.kind === "bidi_names" ? "danger" : ""}>
              <AlertIcon size={16} /> <span>{warningText(t, warning)}</span>
            </li>
          ))}
        </ul>
      )}

      <div className="save-to">
        <span className="save-label">{t("offer.save_to")}</span>
        <span className="save-path" title={folder}><bdi>{folder}</bdi></span>
        <button type="button" className="link-btn" onClick={() => void chooseFolder()}>{t("common.change")}</button>
      </div>
      {shortfall > 0 && (
        <p className="inline-error" role="alert"><AlertIcon size={16} /> {t("offer.shortfall", { size: formatSize(shortfall, lang) })}</p>
      )}

      <footer className="card-actions">
        <button type="button" className="btn" disabled={busy} onClick={() => void run((b) => b.respondOffer(offer.id, false))}>
          {t("offer.decline")}
        </button>
        <button type="button" className="btn primary" disabled={busy || shortfall > 0} onClick={() => void accept()}>
          <CheckIcon size={16} /> {t("offer.accept")}
        </button>
      </footer>
    </article>
  );
}

function IssueList({ issues, truncated }: { issues: Issue[]; truncated: boolean }) {
  const { t } = useTranslation();
  return (
    <ul className="issues">
      {issues.map((issue, index) => (
        <li key={`${issue.path}-${index}`} className={issue.kind}>
          <span className={`pill ${issue.kind}`}>{t(`transfer.kind_${issue.kind}`)}</span>
          <div>
            <bdi className="issue-path">{issue.path}</bdi>
            <p>
              {t(`reason.${issue.reason}`, { defaultValue: issue.reason })}
              {issue.kind === "renamed" && issue.detail && issue.reason === "unsafe_name" && <> ({t("transfer.renamed_from", { name: issue.detail })})</>}
              {issue.kind === "renamed" && issue.detail && issue.reason === "kept_both" && <> ({t("transfer.saved_as", { name: issue.detail })})</>}
            </p>
          </div>
        </li>
      ))}
      {truncated && <li className="more">{t("transfer.items_truncated")}</li>}
    </ul>
  );
}

export function TransferCard({ transfer }: { transfer: TransferView }) {
  const { t } = useTranslation();
  const { lang, run, errorText } = useStore();
  const counts = useCounts();
  const [details, setDetails] = useState(false);
  const sending = transfer.direction === "send";
  const final = ["done", "declined", "cancelled", "failed"].includes(transfer.state);
  const problems = transfer.files_failed + transfer.files_skipped;
  const label = transfer.top_level > 1
    ? `${transfer.label} ${t("common.and_more", { count: transfer.top_level - 1 })}`
    : transfer.label;

  let status: string;
  if (transfer.state === "active") {
    status = t("transfer.state_active", { done: formatSize(transfer.bytes_done, lang), total: formatSize(transfer.bytes_total, lang) });
  } else if (transfer.state === "paused") {
    status = t(`transfer.paused_${transfer.paused_by === "none" ? "local" : transfer.paused_by}`);
  } else if (transfer.state === "done" && problems > 0) {
    status = t("transfer.state_done_partial");
  } else {
    status = t(`transfer.state_${transfer.state}`);
  }
  const tone = transfer.state === "done" ? (problems > 0 ? "muted" : "ok")
    : transfer.state === "failed" ? "danger" : transfer.state === "paused" || final ? "muted" : "accent";

  return (
    <article className={`card transfer ${transfer.state}`}>
      <header className="card-head">
        <span className={`badge-icon ${sending ? "send" : "receive"}`}>
          {sending ? <UploadIcon size={18} /> : <DownloadIcon size={18} />}
        </span>
        <div className="grow">
          <h3><span className="eyebrow">{sending ? t("transfer.sending") : t("transfer.receiving")}</span> <bdi>{label}</bdi></h3>
          <p className="card-sub">
            {transfer.files_total > 0 && <>{counts(transfer.files_total, transfer.folders_total)} · {formatSize(transfer.bytes_total, lang)}</>}
          </p>
        </div>
        {!final && (
          <div className="card-tools">
            {transfer.state === "active" && (
              <button type="button" className="icon-btn" aria-label={t("transfer.pause")} title={t("transfer.pause")}
                onClick={() => void run((b) => b.pauseTransfer(transfer.id))}><PauseIcon size={18} /></button>
            )}
            {transfer.state === "paused" && transfer.paused_by !== "peer" && (
              <button type="button" className="icon-btn" aria-label={t("transfer.resume")} title={t("transfer.resume")}
                onClick={() => void run((b) => b.resumeTransfer(transfer.id))}><PlayIcon size={18} /></button>
            )}
            <button type="button" className="icon-btn" aria-label={t("transfer.cancel")} title={t("transfer.cancel")}
              onClick={() => void run((b) => b.cancelTransfer(transfer.id))}><XIcon size={18} /></button>
          </div>
        )}
      </header>

      {(transfer.state === "active" || transfer.state === "paused" || transfer.state === "done") && (
        <ProgressBar value={transfer.state === "done" ? 1 : transfer.bytes_done} max={transfer.state === "done" ? 1 : transfer.bytes_total}
          label={label} tone={tone} />
      )}
      {(transfer.state === "preparing" || transfer.state === "offered") && <div className="progress indeterminate" aria-hidden="true"><div className="progress-fill" /></div>}

      <div className="transfer-status" aria-live="polite">
        <span className={`status ${tone}`}>{status}</span>
        {transfer.state === "active" && transfer.speed > 0 && (
          <span className="muted">
            {t("transfer.speed", { speed: formatSize(transfer.speed, lang) })}
            {transfer.eta_secs !== null && <> · {t("transfer.eta", { time: formatDuration(transfer.eta_secs, lang) })}</>}
          </span>
        )}
        {transfer.state === "done" && (
          <span className="muted">
            {t(sending ? "transfer.summary_sent" : "transfer.summary_done", { files: counts(transfer.files_done, 0) })}
            {transfer.files_skipped > 0 && <> · {t("transfer.summary_skipped", { count: transfer.files_skipped })}</>}
            {transfer.files_failed > 0 && <> · {t("transfer.summary_failed", { count: transfer.files_failed })}</>}
          </span>
        )}
      </div>
      {transfer.error && transfer.state !== "cancelled" && <p className="inline-error"><AlertIcon size={16} /> {errorText(transfer.error)}</p>}

      {(final || transfer.issues.length > 0) && (transfer.folder || transfer.issues.length > 0) && (
        <footer className="card-actions start">
          {transfer.folder && transfer.state === "done" && transfer.files_done > 0 && (
            <button type="button" className="btn small" onClick={() => void run((b) => b.openFolder(transfer.folder ?? ""))}>
              <FolderIcon size={15} /> {t("transfer.show_in_folder")}
            </button>
          )}
          {transfer.issues.length > 0 && (
            <button type="button" className="btn small" aria-expanded={details} onClick={() => setDetails(!details)}>
              {details ? t("transfer.hide_details") : `${t("transfer.details")} (${transfer.issues.length}${transfer.issues_truncated ? "+" : ""})`}
            </button>
          )}
        </footer>
      )}
      {details && <IssueList issues={transfer.issues} truncated={transfer.issues_truncated} />}
    </article>
  );
}

export function TextBubble({ message, peerName }: { message: TextView; peerName: string }) {
  const { t } = useTranslation();
  const { lang } = useStore();
  const [link, setLink] = useState<string | null>(null);
  return (
    <div className={`bubble-row ${message.from_peer ? "peer" : "own"}`}>
      <div className="bubble">
        <div className="bubble-meta">
          <span>{message.from_peer ? peerName : t("session.you")}</span>
          <time dateTime={new Date(message.at).toISOString()}>{formatTime(message.at, lang)}</time>
        </div>
        <p className="bubble-text">
          {splitLinks(message.text).map((part, index) =>
            part.link ? (
              // Links never open on their own: a click shows the full address first.
              <button key={index} type="button" className="link-btn inline" onClick={() => setLink(part.text)}>
                <LinkIcon size={13} /> <bdi>{part.text}</bdi>
              </button>
            ) : <span key={index}>{part.text}</span>,
          )}
        </p>
        <CopyButton text={message.text} className="btn tiny" />
      </div>
      {link && (
        <Modal title={t("session.open_link_title")} onClose={() => setLink(null)}
          actions={<>
            <button type="button" className="btn" onClick={() => setLink(null)}>{t("common.close")}</button>
            <CopyButton text={link} label={t("session.copy_link")} className="btn primary" />
          </>}>
          <p>{t("session.open_link_text")}</p>
          <p className="link-preview"><bdi>{link}</bdi></p>
        </Modal>
      )}
    </div>
  );
}
