import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Spinner, Toasts } from "./components/Basics";
import { HistoryIcon, HomeIcon, Logo, SessionIcon, SettingsIcon } from "./components/Icons";
import { useStore } from "./lib/store";
import { Connecting, Ended, EnterCode, Home, Safety, ShowCode } from "./screens/Flow";
import { History, SettingsScreen, Welcome } from "./screens/Pages";
import { Session } from "./screens/Session";

type Tab = "main" | "history" | "settings";

// Lets a link open on a given page (used for screenshots of the demo data).
const requestedTab = new URLSearchParams(window.location.search).get("tab");
const initialTab: Tab = requestedTab === "history" || requestedTab === "settings" ? requestedTab : "main";

export default function App() {
  const { t } = useTranslation();
  const { ready, settings, snapshot } = useStore();
  const [tab, setTab] = useState<Tab>(initialTab);
  const [entering, setEntering] = useState(false);
  const wasGuest = useRef(false);
  const session = snapshot.session;

  // Anything that happens in the session brings its screen forward.
  const previous = useRef(session.state);
  useEffect(() => {
    if (session.state !== previous.current) {
      if (session.state === "joining") wasGuest.current = true;
      if (session.state === "creating_code") wasGuest.current = false;
      if (session.state !== "idle") { setTab("main"); setEntering(false); }
      previous.current = session.state;
    }
  }, [session.state]);
  // A new offer needs a decision: show the session.
  const offers = snapshot.offers.length;
  useEffect(() => { if (offers > 0) setTab("main"); }, [offers]);

  if (!ready || !settings) {
    return <div className="boot" role="status"><Spinner size={26} /><span className="visually-hidden">{t("common.loading")}</span></div>;
  }
  if (!settings.onboarded) {
    return <><Welcome /><Toasts /></>;
  }

  const inSession = session.state === "active" || session.state === "reconnecting";
  const busy = session.state !== "idle";

  let main;
  switch (session.state) {
    case "creating_code":
    case "showing_code":
      main = <ShowCode session={session} />;
      break;
    case "joining":
    case "securing":
    case "connecting":
      main = <Connecting state={session.state} />;
      break;
    case "verify_safety":
      main = <Safety session={session} />;
      break;
    case "active":
    case "reconnecting":
      main = <Session session={session} />;
      break;
    case "ended":
      main = <Ended session={session} wasGuest={wasGuest.current} onEnter={() => setEntering(true)} />;
      break;
    default:
      main = entering
        ? <EnterCode onBack={() => setEntering(false)} />
        : <Home onEnter={() => setEntering(true)} onHistory={() => setTab("history")} />;
  }

  const tabs: Array<{ id: Tab; label: string; icon: React.ReactNode }> = [
    { id: "main", label: inSession ? t("nav.session") : t("nav.home"), icon: inSession ? <SessionIcon size={22} /> : <HomeIcon size={22} /> },
    { id: "history", label: t("nav.history"), icon: <HistoryIcon size={22} /> },
    { id: "settings", label: t("nav.settings"), icon: <SettingsIcon size={22} /> },
  ];

  return (
    <div className="shell">
      <nav className="rail" aria-label={t("nav.label")}>
        <div className="rail-logo"><Logo size={36} /></div>
        {tabs.map((item) => (
          <button key={item.id} type="button" className={`rail-btn ${tab === item.id ? "current" : ""}`}
            aria-current={tab === item.id ? "page" : undefined} onClick={() => setTab(item.id)}>
            <span className="rail-icon">
              {item.icon}
              {item.id === "main" && busy && tab !== "main" && <span className="dot" />}
            </span>
            <span className="rail-label">{item.label}</span>
          </button>
        ))}
      </nav>
      <main className="content" id="main">
        {tab === "main" && main}
        {tab === "history" && <History />}
        {tab === "settings" && <SettingsScreen />}
      </main>
      <Toasts />
    </div>
  );
}
