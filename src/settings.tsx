import { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Settings } from "./components/Settings";
import {
  getClaudeDiagnostics,
  getSettings,
  setPreferences,
} from "./desktop/settings";
import type {
  Diagnostics,
  Preferences,
  SettingsView,
} from "./desktop/settings";
import "./styles.css";

function message(error: unknown) {
  return typeof error === "string" ? error : "Não foi possível salvar.";
}

function App() {
  const [view, setView] = useState<SettingsView | null>(null);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const recheck = useCallback(() => {
    void getClaudeDiagnostics().then(setDiagnostics, console.error);
  }, []);

  useEffect(() => {
    void getSettings().then(setView, (failure: unknown) =>
      setError(message(failure)),
    );
    recheck();
    // Monitors may be attached or removed while the window is open.
    const timer = setInterval(() => {
      void getSettings().then(setView, console.error);
    }, 5_000);
    return () => clearInterval(timer);
  }, [recheck]);

  const save = (preferences: Preferences) => {
    setSaving(true);
    setError(null);
    void setPreferences(preferences)
      .then(setView, (failure: unknown) => setError(message(failure)))
      .finally(() => setSaving(false));
  };

  if (!view) {
    return error ? (
      <p className="settings-error" role="alert">
        {error}
      </p>
    ) : null;
  }
  return (
    <Settings
      view={view}
      diagnostics={diagnostics}
      saving={saving}
      error={error}
      now={new Date()}
      onSave={save}
      onRecheck={recheck}
    />
  );
}

createRoot(document.getElementById("root")!).render(<App />);
