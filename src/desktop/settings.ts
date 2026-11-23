import { invoke } from "@tauri-apps/api/core";
import type { Issue, Provider } from "../usage/model";
import type { Side } from "./railGeometry";

// Mirrors src-tauri/src/preferences.rs, settings.rs and usage/model.rs.

export interface MonitorChoice {
  id: string;
  name: string;
}

export interface Preferences {
  version: number;
  side: Side;
  /** `null`: the primary monitor. */
  monitor: MonitorChoice | null;
  providers: { codex: boolean; claudeCode: boolean };
  launchAtStartup: boolean;
}

export interface Monitor {
  id: string;
  name: string;
  primary: boolean;
  width: number;
  height: number;
  scale: number;
}

export type LoadStatus =
  "default" | "loaded" | "recovered" | "newerVersion" | "unavailable";

export type StartupStatus =
  "enabled" | "disabled" | "otherLocation" | "unknown";

export interface SettingsView {
  preferences: Preferences;
  status: LoadStatus;
  monitors: Monitor[];
  placement: { monitor: string | null; fallback: boolean };
  sideOverride: boolean;
  startup: StartupStatus;
  version: string;
  demo: boolean;
}

export type DiagnosticState = "ok" | "info" | "warning" | "error";

export interface DiagnosticRow {
  label: string;
  state: DiagnosticState;
  detail: string;
  at: string | null;
}

export interface Diagnostics {
  provider: Provider;
  rows: DiagnosticRow[];
  issue: Issue | null;
  checkedAt: string | null;
}

export const getSettings = () => invoke<SettingsView>("get_settings");
export const setPreferences = (preferences: Preferences) =>
  invoke<SettingsView>("set_preferences", { preferences });
export const getClaudeDiagnostics = () =>
  invoke<Diagnostics | null>("get_claude_diagnostics");

export interface MonitorOption {
  value: string;
  label: string;
  disconnected: boolean;
}

/** `value` of the primary-monitor option. */
export const primaryValue = "primary";

/**
 * Options of the monitor select. A preferred monitor that is disconnected
 * stays listed, so the choice is visible and kept until reconnection.
 */
export function monitorOptions(view: SettingsView): MonitorOption[] {
  const options: MonitorOption[] = [
    { value: primaryValue, label: "Monitor principal", disconnected: false },
    ...view.monitors.map((monitor) => ({
      value: monitor.id,
      label: [
        monitor.name,
        `${monitor.width} × ${monitor.height}`,
        `${Math.round(monitor.scale * 100)}%`,
        monitor.primary ? "principal" : null,
      ]
        .filter(Boolean)
        .join(" · "),
      disconnected: false,
    })),
  ];
  const wanted = view.preferences.monitor;
  if (wanted && !view.monitors.some((monitor) => monitor.id === wanted.id)) {
    options.push({
      value: wanted.id,
      label: `${wanted.name || "Monitor"} · desconectado`,
      disconnected: true,
    });
  }
  return options;
}

/** The preference for a selected option value. */
export function chooseMonitor(
  view: SettingsView,
  value: string,
): MonitorChoice | null {
  if (value === primaryValue) return null;
  const monitor = view.monitors.find((candidate) => candidate.id === value);
  if (monitor) return { id: monitor.id, name: monitor.name };
  return view.preferences.monitor?.id === value
    ? view.preferences.monitor
    : null;
}

/** Toggles a provider, never hiding the last visible one. */
export function toggleProvider(
  preferences: Preferences,
  provider: Provider,
): Preferences | null {
  const providers = {
    ...preferences.providers,
    [provider]: !preferences.providers[provider],
  };
  if (!providers.codex && !providers.claudeCode) return null;
  return { ...preferences, providers };
}
