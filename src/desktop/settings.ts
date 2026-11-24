import { invoke } from "@tauri-apps/api/core";
import type { Capability, Integration } from "../usage/catalog";
import { providers } from "../usage/catalog";
import type { Issue, Provider } from "../usage/model";
import type { Side } from "./railGeometry";

// Mirrors src-tauri/src/preferences.rs, settings.rs and usage/model.rs.

export interface MonitorChoice {
  id: string;
  name: string;
}

export interface ProviderChoice {
  /** Shown on the rail. A disabled provider reads nothing and alerts nothing. */
  enabled: boolean;
  /** The quota drawn by the ring, or null for the most consumed one. */
  metric: string | null;
  /** Alerts for this provider (when notifications are on). */
  alerts: boolean;
}

export interface QuietHours {
  enabled: boolean;
  /** Local minutes since midnight. `start > end` crosses midnight. */
  start: number;
  end: number;
}

export interface Notifications {
  enabled: boolean;
  thresholds: number[];
  renewal: boolean;
  auth: boolean;
  reconnect: boolean;
  quiet: QuietHours;
}

export interface Preferences {
  version: number;
  side: Side;
  /** `null`: the primary monitor. */
  monitor: MonitorChoice | null;
  providers: Record<Provider, ProviderChoice>;
  launchAtStartup: boolean;
  railVisible: boolean;
  notifications: Notifications;
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
  | "default"
  | "loaded"
  | "migrated"
  | "recovered"
  | "newerVersion"
  | "unavailable";

export type StartupStatus =
  "enabled" | "disabled" | "otherLocation" | "unknown";

export interface MetricOption {
  id: string;
  label: string;
  durationMinutes: number | null;
}

export interface ProviderView {
  id: Provider;
  name: string;
  vendor: string;
  integration: Integration;
  capabilities: Capability[];
  reconnect: string;
  status: "available" | "stale" | "unavailable";
  issue: Issue | null;
  account: string | null;
  plan: string | null;
  updatedAt: string | null;
  checkedAt: string | null;
  metrics: MetricOption[];
  metricMissing: boolean;
}

export interface SettingsView {
  preferences: Preferences;
  status: LoadStatus;
  monitors: Monitor[];
  placement: { monitor: string | null; fallback: boolean; shown: number };
  sideOverride: boolean;
  startup: StartupStatus;
  /** The registry disagrees with the saved start-up preference. */
  startupMismatch: boolean;
  version: string;
  demo: boolean;
  providers: ProviderView[];
  thresholds: number[];
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
export const getDiagnostics = (provider: Provider) =>
  invoke<Diagnostics | null>("get_diagnostics", { provider });
export const sendTestNotification = () =>
  invoke<void>("send_test_notification");

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

/** The preferences with one provider's choice changed. */
export function updateProvider(
  preferences: Preferences,
  provider: Provider,
  change: Partial<ProviderChoice>,
): Preferences {
  return {
    ...preferences,
    providers: {
      ...preferences.providers,
      [provider]: { ...preferences.providers[provider], ...change },
    },
  };
}

/** Number of providers shown on the rail. Zero hides the rail. */
export function enabledCount(preferences: Preferences) {
  return providers.filter((provider) => preferences.providers[provider].enabled)
    .length;
}

/** `value` of the automatic metric option. */
export const automaticMetric = "auto";

export interface MetricChoice {
  value: string;
  label: string;
  missing: boolean;
}

/**
 * Options of a provider's metric select: automatic, then every window of its
 * latest reading. A chosen metric that is not in the reading stays listed as
 * unavailable, so the choice never changes behind the user's back.
 */
export function metricOptions(
  view: ProviderView,
  selected: string | null,
): MetricChoice[] {
  const options: MetricChoice[] = [
    {
      value: automaticMetric,
      label: "Automática · cota mais consumida",
      missing: false,
    },
    ...view.metrics.map((metric) => ({
      value: metric.id,
      label: metric.label,
      missing: false,
    })),
  ];
  if (selected && !view.metrics.some((metric) => metric.id === selected)) {
    options.push({
      value: selected,
      label: `${selected} · indisponível`,
      missing: true,
    });
  }
  return options;
}

export function chooseMetric(value: string): string | null {
  return value === automaticMetric ? null : value;
}

/** `HH:MM` for local minutes since midnight. */
export function formatClock(minutes: number) {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return `${String(hours).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
}

/** Local minutes since midnight for an `HH:MM` value, or null. */
export function parseClock(value: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!match) return null;
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  if (hours > 23 || minutes > 59) return null;
  return hours * 60 + minutes;
}

/** Toggles one alert threshold, keeping the list sorted. */
export function toggleThreshold(thresholds: readonly number[], value: number) {
  const next = thresholds.includes(value)
    ? thresholds.filter((threshold) => threshold !== value)
    : [...thresholds, value];
  return next.sort((a, b) => a - b);
}
