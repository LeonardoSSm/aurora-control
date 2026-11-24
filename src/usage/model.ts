// Mirrors src-tauri/src/usage/model.rs. Never carries credentials.

import type { Provider } from "./catalog";

export type { Provider } from "./catalog";
export { providerNames, providers } from "./catalog";
export type Origin = "api" | "cli" | "sessionLog" | "fixture";
export type QuotaKind = "window" | "credits";

export interface Quota {
  id: string;
  label: string;
  kind: QuotaKind;
  percentUsed: number;
  durationMinutes: number | null;
  /** RFC 3339 with a UTC offset, when known. */
  resetsAt: string | null;
  /** Renewed after the last measurement: `percentUsed` is not a reading. */
  renewed: boolean;
}

/** The metric drawn by the ring, explicitly identified. */
export interface ConsumptionWindow {
  id: string;
  label: string;
  durationMinutes: number | null;
  /** RFC 3339 with a UTC offset, when known. */
  resetsAt: string | null;
  /** Renewed after the last measurement: the percentage is not a reading. */
  renewed: boolean;
}

/** Token activity of local sessions. Not an account quota. */
export interface LocalActivity {
  windowMinutes: number;
  tokens: number;
  sessions: number;
  /** A read limit was reached: the totals cover only part of the logs. */
  partial: boolean;
}

export type Issue =
  | {
      code:
        | "authMissing"
        | "authExpired"
        | "authRejected"
        | "credentialsUnavailable"
        | "credentialsInvalid"
        | "unsupported"
        | "network"
        | "timeout"
        | "malformed";
    }
  | { code: "rateLimited"; retryAt: string | null }
  | { code: "http"; status: number; retryAt: string | null }
  | { code: "sourceBlocked"; reason: BlockReason };

/** Why a provider has no quota source this app may use. */
export type BlockReason = "termsUnverified" | "cliSafety";

type Reading =
  | { status: "available" | "stale"; percentUsed: number }
  | { status: "unavailable" };

export type Usage = Reading & {
  provider: Provider;
  account: string | null;
  plan: string | null;
  source: "demo" | "live";
  origin: Origin | null;
  window: ConsumptionWindow | null;
  /** Remaining quotas, excluding the primary one. */
  quotas: readonly Quota[];
  activity: LocalActivity | null;
  /** Observation time of the last valid reading, RFC 3339. */
  updatedAt: string | null;
  /** Latest attempt, RFC 3339. */
  checkedAt: string | null;
  issue: Issue | null;
  refreshing: boolean;
  /** The metric chosen for the ring, or null for the automatic choice. */
  selectedMetric: string | null;
  /** The chosen metric is absent from the latest reading. */
  metricMissing: boolean;
};

export type Snapshot = readonly Usage[];

/** A snapshot with its publication order (strictly increasing). */
export interface UsageUpdate {
  sequence: number;
  snapshot: Snapshot;
}
