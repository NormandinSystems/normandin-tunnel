// Small building blocks shared by the screens.

import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { useStore } from "../lib/store";
import { CheckIcon, CopyIcon, XIcon } from "./Icons";

export function Toggle({ checked, onChange, label, hint }: {
  checked: boolean; onChange(next: boolean): void; label: string; hint?: string;
}) {
  const id = useId();
  return (
    <div className="row">
      <div className="row-text">
        <label htmlFor={id} className="row-label">{label}</label>
        {hint && <p className="row-hint" id={`${id}-hint`}>{hint}</p>}
      </div>
      <button id={id} type="button" role="switch" aria-checked={checked} aria-describedby={hint ? `${id}-hint` : undefined}
        className={`switch ${checked ? "on" : ""}`} onClick={() => onChange(!checked)}>
        <span className="switch-knob" />
      </button>
    </div>
  );
}

export function Select<T extends string>({ value, onChange, label, options, hint }: {
  value: T; onChange(next: T): void; label: string; options: Array<{ value: T; label: string }>; hint?: string;
}) {
  const id = useId();
  return (
    <div className="row">
      <div className="row-text">
        <label htmlFor={id} className="row-label">{label}</label>
        {hint && <p className="row-hint">{hint}</p>}
      </div>
      <select id={id} className="select" value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
      </select>
    </div>
  );
}

/** A dialog that traps focus, closes on Escape and returns focus when closed. */
export function Modal({ title, children, onClose, actions }: {
  title: string; children: ReactNode; onClose(): void; actions: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const panel = ref.current;
    panel?.querySelector<HTMLElement>("button, [href], input, select, textarea")?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key !== "Tab" || !panel) return;
      const items = panel.querySelectorAll<HTMLElement>("button, [href], input, select, textarea");
      if (items.length === 0) return;
      const [first, last] = [items[0], items[items.length - 1]];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("keydown", onKey); previous?.focus(); };
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={ref}>
        <h2 id={titleId}>{title}</h2>
        <div className="modal-body">{children}</div>
        <div className="modal-actions">{actions}</div>
      </div>
    </div>
  );
}

/** A button that copies text and briefly says so. Nothing is ever copied automatically. */
export function CopyButton({ text, label, className = "btn" }: { text: string; label?: string; className?: string }) {
  const { t } = useTranslation();
  const { api } = useStore();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1600);
    return () => window.clearTimeout(timer);
  }, [copied]);
  return (
    <button type="button" className={className} onClick={() => { void api?.copyText(text).then(() => setCopied(true)); }}>
      {copied ? <CheckIcon size={16} /> : <CopyIcon size={16} />}
      <span aria-live="polite">{copied ? t("common.copied") : (label ?? t("common.copy"))}</span>
    </button>
  );
}

export function ProgressBar({ value, max, label, tone = "accent" }: {
  value: number; max: number; label: string; tone?: "accent" | "muted" | "ok" | "danger";
}) {
  const percent = max <= 0 ? 100 : Math.min(100, Math.max(0, (value / max) * 100));
  return (
    <div className={`progress ${tone}`} role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100}
      aria-valuenow={Math.round(percent)}>
      <div className="progress-fill" style={{ width: `${percent}%` }} />
    </div>
  );
}

export function Toasts() {
  const { toasts, dismissToast } = useStore();
  const { t } = useTranslation();
  return (
    <div className="toasts" aria-live="assertive">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast ${toast.kind}`} role={toast.kind === "error" ? "alert" : "status"}>
          <span>{toast.text}</span>
          <button type="button" className="icon-btn" aria-label={t("common.close")} onClick={() => dismissToast(toast.id)}>
            <XIcon size={16} />
          </button>
        </div>
      ))}
    </div>
  );
}

export function Spinner({ size = 18 }: { size?: number }) {
  return <span className="spinner" style={{ width: size, height: size }} aria-hidden="true" />;
}
