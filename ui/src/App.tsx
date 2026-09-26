import { useState } from "react";
import { SearchScreen } from "./screens/SearchScreen";
import { DrivesScreen } from "./screens/DrivesScreen";
import { ScanScreen } from "./screens/ScanScreen";
import { ReviewScreen } from "./screens/ReviewScreen";
import { EventsScreen } from "./screens/EventsScreen";
import { YearsScreen } from "./screens/YearsScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { runningInTauri } from "./api";

type Section = "search" | "drives" | "review" | "events" | "settings";

/// Five places, named for what you do there. Scan activity used to be a sixth
/// section; it is part of looking after drives, so it lives under Drives.
const NAV: { id: Section; label: string; hint: string }[] = [
  { id: "search", label: "Find", hint: "Search every drive" },
  { id: "drives", label: "Drives", hint: "Your drives and scans" },
  { id: "review", label: "People", hint: "Name the people who matter" },
  { id: "events", label: "Events", hint: "Weddings, shoots, occasions" },
  { id: "settings", label: "Settings", hint: "Backup and more" },
];

/// A filter handed from one screen to another — Events sending you to Search
/// with "just this shoot" already applied.
export interface SearchContext {
  eventId?: string;
  client?: string;
  year?: number;
  label: string;
}

export function App() {
  const [section, setSection] = useState<Section>("search");
  const [drivesTab, setDrivesTab] = useState<"drives" | "scan" | "years">("drives");
  const [context, setContext] = useState<SearchContext | null>(null);
  // A search asked for from another screen ("Find their photographs"). The
  // counter makes asking twice for the same name run it twice.
  const [asked, setAsked] = useState<{ query: string; n: number } | null>(null);
  function findFor(query: string) {
    setContext(null);
    setAsked((prev) => ({ query, n: (prev?.n ?? 0) + 1 }));
    setSection("search");
  }
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
        {section === "search" && (
          <SearchScreen
            context={context}
            onClearContext={() => setContext(null)}
            onGo={(place) => setSection(place)}
            asked={asked}
          />
        )}
        {section === "drives" && (
          <>
            <div className="tabs" role="tablist" aria-label="Drives">
              <button
                role="tab"
                aria-selected={drivesTab === "drives"}
                className={drivesTab === "drives" ? "tab active" : "tab"}
                onClick={() => setDrivesTab("drives")}
              >
                All drives
              </button>
              <button
                role="tab"
                aria-selected={drivesTab === "scan"}
                className={drivesTab === "scan" ? "tab active" : "tab"}
                onClick={() => setDrivesTab("scan")}
              >
                Scan activity
              </button>
              <button
                role="tab"
                aria-selected={drivesTab === "years"}
                className={drivesTab === "years" ? "tab active" : "tab"}
                onClick={() => setDrivesTab("years")}
              >
                Years
              </button>
            </div>
            {drivesTab === "drives" ? (
              <DrivesScreen />
            ) : drivesTab === "scan" ? (
              <ScanScreen />
            ) : (
              <YearsScreen onSearchWithin={searchWithin} />
            )}
          </>
        )}
        {section === "review" && <ReviewScreen onFind={findFor} />}
        {section === "events" && <EventsScreen onSearchWithin={searchWithin} />}
        {section === "settings" && <SettingsScreen />}
      </main>
    </div>
  );
}
