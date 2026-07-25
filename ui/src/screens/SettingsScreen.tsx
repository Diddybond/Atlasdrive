import { useEffect, useState } from "react";
import { api, VerifierCheck } from "../api";

export function SettingsScreen() {
  const [checks, setChecks] = useState<VerifierCheck[]>([]);
  const [doctor, setDoctor] = useState<Record<string, string>>({});
  const [running, setRunning] = useState(false);
  const [exported, setExported] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  async function exportDiagnostics() {
    setExporting(true);
    try {
      setExported(await api.exportDiagnostics());
    } finally {
      setExporting(false);
    }
  }

  async function runChecks() {
    setRunning(true);
    try {
      setChecks(await api.runVerifier());
    } finally {
      setRunning(false);
    }
  }
  useEffect(() => {
    void (async () => {
      setDoctor(await api.doctor());
      await runChecks();
    })();
  }, []);

  const badge = (s: VerifierCheck["status"]) =>
    s === "Pass" ? "ok" : s === "Warn" ? "warn" : "fail";

  return (
    <section aria-labelledby="settings-heading">
      <h1 id="settings-heading">Settings &amp; diagnostics</h1>
      <p className="lede">
        Everything runs on this Mac. These checks confirm your archive is safe, consistent and fully
        offline.
      </p>

      <div className="card">
        <h2>Environment</h2>
        <dl className="kv">
          {Object.entries(doctor).map(([k, v]) => (
            <div key={k}>
              <dt>{k.replace(/_/g, " ")}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="card">
        <div className="row-between">
          <h2>Safety checks</h2>
          <button onClick={runChecks} disabled={running}>
            {running ? "Checking…" : "Run checks"}
          </button>
        </div>
        <ul className="check-list">
          {checks.map((c) => (
            <li key={c.name} className="check-row">
              <span className={`pill ${badge(c.status)}`}>{c.status}</span>
              <span className="check-name">{c.name.replace(/_/g, " ")}</span>
              <span className="check-detail">{c.detail}</span>
            </li>
          ))}
        </ul>
      </div>

      <div className="card">
        <div className="row-between">
          <h2>Share a diagnostics file</h2>
          <button onClick={exportDiagnostics} disabled={exporting}>
            {exporting ? "Writing…" : "Create diagnostics file"}
          </button>
        </div>
        <p className="lede">
          Creates a file you can send with a bug report. It contains counts, version numbers and
          the results of the safety checks — never your file names, folders, dates, tags, people or
          photographs.
        </p>
        {exported && (
          <p className="check-detail" role="status">
            Saved to {exported}
          </p>
        )}
      </div>
    </section>
  );
}
