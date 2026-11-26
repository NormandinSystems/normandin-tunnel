// Numbers, sizes and times, written the way each language expects.

export type Lang = "en" | "fr";

const UNITS: Record<Lang, string[]> = {
  en: ["B", "KB", "MB", "GB", "TB"],
  fr: ["o", "Ko", "Mo", "Go", "To"],
};

export function formatSize(bytes: number, lang: Lang): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1000 && unit < UNITS[lang].length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  const number = new Intl.NumberFormat(lang, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value);
  return `${number} ${UNITS[lang][unit]}`;
}

export function formatCount(count: number, lang: Lang): string {
  return new Intl.NumberFormat(lang).format(count);
}

/** A duration as "45 s", "3 min 20 s" or "2 h 05 min". */
export function formatDuration(totalSeconds: number, lang: Lang): string {
  const seconds = Math.max(0, Math.round(totalSeconds));
  const hour = lang === "fr" ? "h" : "h";
  if (seconds < 60) return `${seconds} s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min ${String(seconds % 60).padStart(2, "0")} s`;
  return `${Math.floor(seconds / 3600)} ${hour} ${String(Math.floor((seconds % 3600) / 60)).padStart(2, "0")} min`;
}

/** A countdown as "9:41". */
export function formatClock(totalSeconds: number): string {
  const seconds = Math.max(0, Math.ceil(totalSeconds));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function formatDate(ms: number, lang: Lang): string {
  return new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "short" }).format(new Date(ms));
}

export function formatTime(ms: number, lang: Lang): string {
  return new Intl.DateTimeFormat(lang, { timeStyle: "short" }).format(new Date(ms));
}

/** How a screen reader should say a connection code: one part at a time. */
export function spokenCode(code: string): string {
  return code.split("-").join(", ");
}

/** How a screen reader should say a safety code: digits in pairs. */
export function spokenSafetyCode(code: string): string {
  const digits = code.replace(/\D/g, "");
  return (digits.match(/.{1,2}/g) ?? []).join(", ");
}

/** Finds http(s) links in text, so they can be shown as links that ask first. */
export function splitLinks(text: string): Array<{ text: string; link: boolean }> {
  const parts: Array<{ text: string; link: boolean }> = [];
  const pattern = /https?:\/\/[^\s<>"']+/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0;
    if (start > last) parts.push({ text: text.slice(last, start), link: false });
    parts.push({ text: match[0], link: true });
    last = start + match[0].length;
  }
  if (last < text.length) parts.push({ text: text.slice(last), link: false });
  return parts;
}
