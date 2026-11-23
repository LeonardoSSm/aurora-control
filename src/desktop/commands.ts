import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Provider, Snapshot, Usage } from "../usage/model";

export interface SummarySelection {
  usage: Usage;
  revision: number;
}

export const getUsage = () => invoke<Snapshot>("get_usage");
export const setHover = (provider: Provider | null) =>
  invoke<void>("set_hover", { provider });
export const summaryReady = () => invoke<void>("summary_ready");
export const showSummary = (revision: number) =>
  invoke<void>("show_summary", { revision });

export function onSummarySelection(
  handler: (payload: SummarySelection) => void,
) {
  return getCurrentWebviewWindow().listen<SummarySelection>(
    "summary-selection",
    ({ payload }) => handler(payload),
  );
}
