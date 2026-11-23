export type Provider = "codex" | "claudeCode";

export interface ConsumptionWindow {
  durationMinutes: number;
  /** RFC 3339 with a UTC offset, when known. */
  resetsAt: string | null;
}

type Reading =
  | { status: "available" | "stale"; percentUsed: number }
  | { status: "unavailable" };

export type Usage = Reading & {
  provider: Provider;
  account: string | null;
  source: "demo" | "live";
  window: ConsumptionWindow | null;
  /** Last successful reading as RFC 3339 with a UTC offset. */
  updatedAt: string | null;
};

export type Snapshot = readonly Usage[];

export const providers = ["codex", "claudeCode"] as const;

export const providerNames: Record<Provider, string> = {
  codex: "Codex",
  claudeCode: "Claude Code",
};
