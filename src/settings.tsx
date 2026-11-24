import { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Settings } from "./components/Settings";
import type { TestState } from "./components/Settings";
import {
  getDiagnostics,
  getSettings,
  resetProviderSafety,
  sendTestNotification,
  setPreferences,
} from "./desktop/settings";
import type {
  Diagnostics,
  Preferences,
  SettingsView,
} from "./desktop/settings";
import type { Provider } from "./usage/model";
import "./styles.css";

function message(error: unknown) {
  return typeof error === "string" ? error : "Não foi possível salvar.";
}

function App() {
  const [view, setView] = useState<SettingsView | null>(null);
  const [diagnostics, setDiagnostics] = useState<
    Partial<Record<Provider, Diagnostics | null>>
  >({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<TestState>("idle");
  const [now, setNow] = useState(() => new Date());

  const diagnose = useCallback((provider: Provider) => {
    void getDiagnostics(provider).then(
      (result) =>
        setDiagnostics((current) => ({ ...current, [provider]: result })),
      console.error,
    );
  }, []);

  useEffect(() => {
    void getSettings().then(setView, (failure: unknown) =>
      setError(message(failure)),
    );
    // Monitors, connection states and readings change while the window is
    // open: the view is refreshed from the backend (no provider is queried).
    const timer = setInterval(() => {
      setNow(new Date());
      void getSettings().then(setView, console.error);
    }, 5_000);
    return () => clearInterval(timer);
  }, []);

  const save = (preferences: Preferences) => {
    setSaving(true);
    setError(null);
    void setPreferences(preferences)
      .then(
        (next) => {
          setView(next);
          // Enabling or disabling a provider changes what may be diagnosed.
          setDiagnostics({});
        },
        (failure: unknown) => {
          setError(message(failure));
          // A failed save may leave the registry changed: show what is
          // actually stored rather than the state before the attempt.
          void getSettings().then(setView, console.error);
        },
      )
      .finally(() => setSaving(false));
  };

  const resetSafety = (provider: Provider) => {
    setSaving(true);
    setError(null);
    void resetProviderSafety(provider)
      .then(
        (result) => {
          setDiagnostics((current) => ({ ...current, [provider]: result }));
          void getSettings().then(setView, console.error);
        },
        (failure: unknown) => setError(message(failure)),
      )
      .finally(() => setSaving(false));
  };

  const testNotification = () => {
    setTest("sending");
    void sendTestNotification().then(
      () => setTest("sent"),
      (failure: unknown) => {
        console.error(failure);
        setTest("failed");
      },
    );
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
      now={now}
      test={test}
      onSave={save}
      onDiagnose={diagnose}
      onResetSafety={resetSafety}
      onTestNotification={testNotification}
    />
  );
}

createRoot(document.getElementById("root")!).render(<App />);
