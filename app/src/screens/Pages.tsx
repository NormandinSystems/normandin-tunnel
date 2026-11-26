// History, settings and the first-run welcome.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Modal, Select, Toggle } from "../components/Basics";
import { DownloadIcon, FolderIcon, Logo, UploadIcon } from "../components/Icons";
import { formatCount, formatDate, formatSize } from "../lib/format";
import { useStore } from "../lib/store";
import type { Settings } from "../lib/types";

export function History() {
  const { t } = useTranslation();
  const { history, lang, settings, run, refreshHistory } = useStore();
  const [confirm, setConfirm] = useState(false);
  useEffect(() => { void refreshHistory(); }, [refreshHistory]);
  const off = settings?.history === "off";
  return (
    <div className="page">
      <header className="page-head row-head">
        <h1>{t("history.title")}</h1>
        {history.length > 0 && <button type="button" className="btn" onClick={() => setConfirm(true)}>{t("history.clear")}</button>}
      </header>
      {off && <p className="empty">{t("history.off")}</p>}
      {!off && history.length === 0 && <p className="empty">{t("history.empty")}</p>}
      {history.length > 0 && (
        <ul className="list">
          {history.map((entry) => (
            <li key={entry.id} className="list-row tall">
              <span className={`badge-icon ${entry.direction}`}>
                {entry.direction === "send" ? <UploadIcon size={16} /> : <DownloadIcon size={16} />}
              </span>
              <div className="grow">
                <p className="list-title">
                  {t(`history.${entry.direction === "send" ? "sent_to" : "received_from"}`, { name: entry.peer })}
                </p>
                <p className="list-sub">
                  {formatDate(entry.at, lang)} · {t("offer.files", { count: entry.files, formatted: formatCount(entry.files, lang) })} · {formatSize(entry.bytes, lang)}
                </p>
                {entry.names.length > 0 && <p className="list-names"><bdi>{entry.names.map((name) => name.replace(/\/$/, "")).join(", ")}</bdi></p>}
              </div>
              <div className="list-end">
                <span className={`pill ${entry.result}`}>{t(`history.result_${entry.result}`)}</span>
                {entry.folder && entry.direction === "receive" && (
                  <button type="button" className="btn tiny" onClick={() => void run((b) => b.openFolder(entry.folder ?? ""))}>
                    <FolderIcon size={14} /> {t("transfer.show_in_folder")}
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      {!off && settings && settings.history !== "forever" && history.length > 0 && (
        <p className="muted small">{t("history.kept", { period: t(`settings.history_${settings.history}`) })}</p>
      )}
      {confirm && (
        <Modal title={t("history.clear")} onClose={() => setConfirm(false)}
          actions={<>
            <button type="button" className="btn" onClick={() => setConfirm(false)}>{t("common.cancel")}</button>
            <button type="button" className="btn danger" onClick={() => { setConfirm(false); void run(async (b) => { await b.clearHistory(); await refreshHistory(); }); }}>{t("history.clear")}</button>
          </>}>
          <p>{t("history.clear_confirm")}</p>
        </Modal>
      )}
    </div>
  );
}

function TextSetting({ label, hint, value, onCommit, multiline, placeholder }: {
  label: string; hint?: string; value: string; onCommit(next: string): void; multiline?: boolean; placeholder?: string;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const id = `setting-${label.replace(/\W+/g, "-")}`;
  const commit = () => { if (draft !== value) onCommit(draft); };
  return (
    <div className="row column">
      <label htmlFor={id} className="row-label">{label}</label>
      {multiline
        ? <textarea id={id} className="input" rows={2} value={draft} placeholder={placeholder} spellCheck={false} onChange={(e) => setDraft(e.target.value)} onBlur={commit} />
        : <input id={id} className="input" value={draft} placeholder={placeholder} spellCheck={false} maxLength={300}
            onChange={(e) => setDraft(e.target.value)} onBlur={commit} onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />}
      {hint && <p className="row-hint">{hint}</p>}
    </div>
  );
}

function FolderSetting({ label, value, onPick }: { label: string; value: string; onPick(path: string): void }) {
  const { t } = useTranslation();
  const { api } = useStore();
  return (
    <div className="row">
      <div className="row-text">
        <span className="row-label">{label}</span>
        <p className="row-hint path"><bdi>{value}</bdi></p>
      </div>
      <button type="button" className="btn" onClick={() => { void api?.pickFolder(label).then((path) => { if (path) onPick(path); }); }}>
        <FolderIcon size={16} /> {t("common.change")}
      </button>
    </div>
  );
}

export function SettingsScreen() {
  const { t } = useTranslation();
  const { settings, saveSettings, info } = useStore();
  const [serversOpened, setAdvanced] = useState<boolean | null>(null);
  if (!settings) return null;
  // Open by itself when own servers are already set, so they are never hidden.
  const advanced = serversOpened ?? (settings.rendezvous_url !== "" || settings.relay_urls.length > 0);
  const set = (patch: Partial<Settings>) => void saveSettings({ ...settings, ...patch });
  return (
    <div className="page">
      <header className="page-head"><h1>{t("settings.title")}</h1></header>

      <section className="panel settings" aria-labelledby="set-general">
        <h2 id="set-general">{t("settings.general")}</h2>
        <TextSetting label={t("settings.device_name")} hint={t("welcome.device_name_hint")} value={settings.device_name} onCommit={(device_name) => set({ device_name })} />
        <Select label={t("settings.language")} value={settings.language} onChange={(language) => set({ language })}
          options={[{ value: "system", label: t("settings.language_system") }, { value: "en", label: "English" }, { value: "fr", label: "Français" }]} />
        <Select label={t("settings.theme")} value={settings.theme} onChange={(theme) => set({ theme })}
          options={[{ value: "system", label: t("settings.theme_system") }, { value: "light", label: t("settings.theme_light") }, { value: "dark", label: t("settings.theme_dark") }]} />
        <Toggle label={t("settings.notifications")} hint={t("settings.notifications_hint")} checked={settings.notifications} onChange={(notifications) => set({ notifications })} />
      </section>

      <section className="panel settings" aria-labelledby="set-receiving">
        <h2 id="set-receiving">{t("settings.receiving")}</h2>
        <FolderSetting label={t("settings.save_folder")} value={settings.save_dir} onPick={(save_dir) => set({ save_dir })} />
        <Toggle label={t("settings.ask_where")} checked={settings.ask_where_to_save} onChange={(ask_where_to_save) => set({ ask_where_to_save })} />
        <Select label={t("settings.conflict")} value={settings.conflict_rule} onChange={(conflict_rule) => set({ conflict_rule })}
          options={[{ value: "keep_both", label: t("settings.conflict_keep_both") }, { value: "replace", label: t("settings.conflict_replace") }, { value: "skip", label: t("settings.conflict_skip") }]} />
        <Toggle label={t("settings.keep_times")} checked={settings.keep_modified_times} onChange={(keep_modified_times) => set({ keep_modified_times })} />
        <Toggle label={t("settings.keep_exec")} hint={t("settings.keep_exec_hint")} checked={settings.keep_executable} onChange={(keep_executable) => set({ keep_executable })} />
        <Toggle label={t("settings.mark_downloaded")} hint={t("settings.mark_downloaded_hint")} checked={settings.mark_downloaded} onChange={(mark_downloaded) => set({ mark_downloaded })} />
      </section>

      <section className="panel settings" aria-labelledby="set-privacy">
        <h2 id="set-privacy">{t("settings.privacy")}</h2>
        <Select label={t("settings.history")} value={settings.history} onChange={(history) => set({ history })}
          options={(["off", "days7", "days30", "days90", "forever"] as const).map((value) => ({ value, label: t(`settings.history_${value}`) }))} />
        <Toggle label={t("settings.history_names")} checked={settings.history_names} onChange={(history_names) => set({ history_names })} />
        <p className="row-hint note">{t("settings.privacy_note")}</p>
      </section>

      <section className="panel settings" aria-labelledby="set-advanced">
        <h2 id="set-advanced">{t("settings.advanced")}</h2>
        <p className="row-hint note">{t("settings.pairing_note")}</p>
        <Toggle label={t("settings.own_servers")} checked={advanced} onChange={setAdvanced} />
        {advanced && (
          <>
            <TextSetting label={t("settings.rendezvous")} hint={t("settings.rendezvous_hint")} value={settings.rendezvous_url}
              placeholder="wss://rendezvous.example.com/v1" onCommit={(rendezvous_url) => set({ rendezvous_url })} />
            <TextSetting multiline label={t("settings.relays")} hint={t("settings.relays_hint")} value={settings.relay_urls.join("\n")}
              placeholder="https://relay.example.com" onCommit={(text) => set({ relay_urls: text.split(/\s+/).filter(Boolean) })} />
          </>
        )}
        <Select label={t("settings.max_entries")} value={String(settings.max_entries)} onChange={(value) => set({ max_entries: Number(value) })}
          options={[100_000, 1_000_000, 10_000_000].map((n) => ({ value: String(n), label: new Intl.NumberFormat().format(n) }))} />
        <Toggle label={t("settings.debug")} hint={t("settings.debug_hint")} checked={settings.debug_logging} onChange={(debug_logging) => set({ debug_logging })} />
      </section>

      <section className="panel settings about" aria-labelledby="set-about">
        <h2 id="set-about">{t("settings.about")}</h2>
        <div className="about-head">
          <Logo size={44} />
          <div>
            <p className="about-name">Normandin Tunnel</p>
            <p className="muted small">{t("settings.version", { version: info?.version ?? "" })}</p>
          </div>
        </div>
        {info && <p className="row-hint">{t("settings.data_folder")}: <span className="path"><bdi>{info.data_dir}</bdi></span></p>}
        <p className="row-hint">{t("settings.credits")}</p>
      </section>
    </div>
  );
}

export function Welcome() {
  const { t } = useTranslation();
  const { settings, saveSettings, api } = useStore();
  const [name, setName] = useState(settings?.device_name ?? "");
  const [folder, setFolder] = useState(settings?.save_dir ?? "");
  if (!settings) return null;
  const start = () => void saveSettings({ ...settings, device_name: name.trim() || settings.device_name, save_dir: folder, onboarded: true });
  return (
    <div className="welcome">
      <div className="welcome-card">
        <Logo size={56} />
        <h1>{t("welcome.title")}</h1>
        <p className="lead">{t("welcome.intro")}</p>
        <form onSubmit={(event) => { event.preventDefault(); start(); }}>
          <div className="field">
            <label htmlFor="welcome-name">{t("welcome.device_name")}</label>
            <input id="welcome-name" className="input" value={name} maxLength={64} onChange={(event) => setName(event.target.value)} autoFocus />
            <p className="row-hint">{t("welcome.device_name_hint")}</p>
          </div>
          <div className="field">
            <span className="field-label">{t("welcome.save_folder")}</span>
            <div className="folder-pick">
              <span className="path"><bdi>{folder}</bdi></span>
              <button type="button" className="btn small" onClick={() => { void api?.pickFolder(t("welcome.save_folder")).then((path) => { if (path) setFolder(path); }); }}>
                <FolderIcon size={15} /> {t("common.change")}
              </button>
            </div>
          </div>
          <div className="field">
            <label htmlFor="welcome-lang">{t("welcome.language")}</label>
            <select id="welcome-lang" className="select wide" value={settings.language}
              onChange={(event) => void saveSettings({ ...settings, language: event.target.value as Settings["language"] })}>
              <option value="system">{t("settings.language_system")}</option>
              <option value="en">English</option>
              <option value="fr">Français</option>
            </select>
          </div>
          <p className="notice">{t("welcome.firewall")}</p>
          <button type="submit" className="btn primary large wide">{t("welcome.start")}</button>
        </form>
      </div>
    </div>
  );
}
