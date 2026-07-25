import { useState } from "react";
import { SearchScreen } from "./screens/SearchScreen";
import { DrivesScreen } from "./screens/DrivesScreen";
import { ScanScreen } from "./screens/ScanScreen";
import { ReviewScreen } from "./screens/ReviewScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { runningInTauri } from "./api";

type Section = "search" | "drives" | "review" | "scan" | "settings";

const NAV: { id: Section; label: string; hint: string }[] = [
  { id: "search", label: "Search", hint: "Find any photograph" },
  { id: "drives", label: "Drives", hint: "Your numbered drives" },
  { id: "review", label: "Review", hint: "Name people, check suggestions" },
  { id: "scan", label: "Scan activity", hint: "Indexing progress" },
  { id: "settings", label: "Settings", hint: "Diagnostics and safety" },
];

export function App() {
  const [section, setSection] = useState<Section>("search");

  return (
    <div className="app">
      <nav className="sidebar" aria-label="Main sections">
        <div className="brand">
          <img className="brand-mark" src="./atlasdrive-mark.png" alt="" width={36} height={36} />
          <span className="brand-text">
            <span className="brand-name">AtlasDrive</span>
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
        {section === "search" && <SearchScreen />}
        {section === "drives" && <DrivesScreen />}
        {section === "review" && <ReviewScreen />}
        {section === "scan" && <ScanScreen />}
        {section === "settings" && <SettingsScreen />}
      </main>
    </div>
  );
}
