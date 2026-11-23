// Mirrors src-tauri/src/usage/model.rs. Never carries credentials.

export type Provider = "codex" | "claudeCode";
export type Origin = "api" | "sessionLog" | "fixture";
export type QuotaKind = "window" | "credits";

export interface Quota {
  id: string;
  label: string;
  kind: QuotaKind;
  percentUsed: number;
  durationMinutes: number | null;
  /** RFC 3339 with a UTC offset, when known. */
  resetsAt: string | null;
}

/** The metric drawn by the ring, explicitly identified. */
export interface ConsumptionWindow {
  id: string;
  label: string;
  durationMinutes: number | null;
  /** RFC 3339 with a UTC offset, when known. */
  resetsAt: string | null;
}

/** Token activity of local sessions. Not an account quota. */
export interface LocalActivity {
  windowMinutes: number;
  tokens: number;
  sessions: number;
}

export type Issue =
  | {
      code:
        | "authMissing"
        | "authExpired"
        | "authRejected"
        | "unsupported"
        | "network"
        | "timeout"
        | "malformed";
    }
  | { code: "rateLimited"; retryAt: string | null }
  | { code: "http"; status: number };

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
};

export type Snapshot = readonly Usage[];

export const providers = ["codex", "claudeCode"] as const;

export const providerNames: Record<Provider, string> = {
  codex: "Codex",
  claudeCode: "Claude Code",
};
