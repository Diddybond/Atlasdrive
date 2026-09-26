import { useEffect, useState } from "react";
import { SearchScreen } from "./screens/SearchScreen";
import { DrivesScreen } from "./screens/DrivesScreen";
import { ScanScreen } from "./screens/ScanScreen";
import { ReviewScreen } from "./screens/ReviewScreen";
import { EventsScreen } from "./screens/EventsScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { api, runningInTauri, Settings } from "./api";
import { backupWarning } from "./lib/backupWarning";

type Section = "search" | "drives" | "review" | "events" | "scan" | "settings";

const NAV: { id: Section; label: string; hint: string }[] = [
  { id: "search", label: "Search", hint: "Find any photograph" },
  { id: "drives", label: "Drives", hint: "Your numbered drives" },
  { id: "review", label: "People", hint: "Name faces, check suggestions" },
  { id: "events", label: "Events", hint: "Weddings, shoots, clients" },
  { id: "scan", label: "Scan activity", hint: "Indexing progress" },
  { id: "settings", label: "Settings", hint: "Diagnostics and safety" },
];

/// A filter handed from one screen to another — Events sending you to Search
/// with "just this shoot" already applied.
export interface SearchContext {
  eventId?: string;
  client?: string;
  label: string;
}

export function App() {
  const [section, setSection] = useState<Section>("search");
  const [context, setContext] = useState<SearchContext | null>(null);
  // Re-read on every change of screen, so a backup made in Settings clears the
  // notice as soon as you leave it — and a backup that stops happening brings
  // it back.
  const [settings, setSettings] = useState<Settings | null>(null);
  useEffect(() => {
    void api.getSettings().then(setSettings, () => setSettings(null));
  }, [section]);
  const warning = section === "settings" ? null : backupWarning(settings);

  /// Jumping to Search with a filter is the only cross-screen navigation in
  /// the app, so it is a callback rather than a router.
  function searchWithin(next: SearchContext) {
    setContext(next);
    setSection("search");
  }

  return (
    <div className="app">
      <nav className="sidebar" aria-label="Main sections">
        <div className="brand">
          <img className="brand-mark" src="./atlasdrive-mark.png" alt="" width={36} height={36} />
          <span className="brand-text">
            {/* Split only so the wordmark can carry two weights; it still reads
                and copies as the single word "AtlasDrive". */}
            <span className="brand-name">
              <span className="wm-atlas">Atlas</span>
              <span className="wm-drive">Drive</span>
            </span>
            <span className="brand-tagline">Your photographs, mapped</span>
          </span>
        </div>
        <ul>
          {NAV.map((item) => (
            <li key={item.id}>
              <button
                className={section === item.id ? "nav-item active" : "nav-item"}
                aria-current={section === item.id ? "page" : undefined}
                onClick={() => setSection(item.id)}
              >
                <span className="nav-label">{item.label}</span>
                <span className="nav-hint">{item.hint}</span>
              </button>
            </li>
          ))}
        </ul>
        {!runningInTauri() && (
          <p className="demo-badge" role="note">
            Demo mode — showing sample data. Connect the app to a drive to index real photographs.
          </p>
        )}
      </nav>

      <main className="content" aria-live="polite">
        {warning && (
          <div className="notice-bar" role="alert">
            <span>{warning}</span>
            <button onClick={() => setSection("settings")}>
              {settings?.backup_destination ? "Back up now" : "Choose a backup folder"}
            </button>
          </div>
        )}
        {section === "search" && (
          <SearchScreen context={context} onClearContext={() => setContext(null)} />
        )}
        {section === "drives" && <DrivesScreen />}
        {section === "review" && <ReviewScreen />}
        {section === "scan" && <ScanScreen />}
        {section === "events" && <EventsScreen onSearchWithin={searchWithin} />}
        {section === "settings" && <SettingsScreen />}
      </main>
    </div>
  );
}
