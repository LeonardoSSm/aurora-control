// The provider catalog, shared with src-tauri/src/usage/catalog.rs through
// catalog.json: one list of providers for both languages.
import entries from "./catalog.json";

export type Provider = "codex" | "claudeCode" | "antigravity" | "grok";

/** How far the provider's data source goes (see docs/provider-compatibility.md). */
export type Integration = "live" | "cli" | "gated";

export type Capability =
  | "subscriptionWindows"
  | "modelQuotas"
  | "credits"
  | "localActivity"
  | "sessionLogFallback"
  | "diagnostics";

export interface CatalogEntry {
  id: Provider;
  name: string;
  vendor: string;
  /** New providers stay off until the user enables them. */
  defaultEnabled: boolean;
  refreshSeconds: number;
  integration: Integration;
  capabilities: readonly Capability[];
  /** What the user does to restore monitoring after a sign-in problem. */
  reconnect: string;
}

/** Every provider, in display order. Checked against the type in tests. */
export const catalog = entries as readonly CatalogEntry[];

export const providers: readonly Provider[] = catalog.map((entry) => entry.id);

export const providerNames = Object.fromEntries(
  catalog.map((entry) => [entry.id, entry.name]),
) as Record<Provider, string>;

export function catalogEntry(provider: Provider): CatalogEntry {
  const entry = catalog.find((candidate) => candidate.id === provider);
  if (!entry) throw new Error(`Unknown provider: ${provider}`);
  return entry;
}
