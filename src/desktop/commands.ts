import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Provider, Snapshot, Usage } from "../usage/model";
import type { RailWidths, Side } from "./railGeometry";

export interface RailPlacement {
  side: Side;
  generation: number;
}

export interface SummarySelection {
  usage: Usage;
  revision: number;
  side: Side;
}

export const getUsage = () => invoke<Snapshot>("get_usage");
export const getRailPlacement = () =>
  invoke<RailPlacement>("get_rail_placement");
export const setHover = (generation: number, provider: Provider | null) =>
  invoke<void>("set_hover", { generation, provider });
export const setRailExpansion = (generation: number, widths: RailWidths) =>
  invoke<boolean>("set_rail_expansion", { generation, widths });
export const summaryReady = () => invoke<void>("summary_ready");
export const showSummary = (revision: number) =>
  invoke<void>("show_summary", { revision });

export function onRailPlacement(handler: (payload: RailPlacement) => void) {
  return getCurrentWebviewWindow().listen<RailPlacement>(
    "rail-placement",
    ({ payload }) => handler(payload),
  );
}

export function onSummarySelection(
  handler: (payload: SummarySelection) => void,
) {
  return getCurrentWebviewWindow().listen<SummarySelection>(
    "summary-selection",
    ({ payload }) => handler(payload),
  );
}
