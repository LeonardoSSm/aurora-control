import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Provider, Usage, UsageUpdate } from "../usage/model";
import type { SelectionOrder } from "./ordering";
import type { RailShifts, Side } from "./railGeometry";

export interface RailPlacement {
  side: Side;
  generation: number;
  /** Visible indicators, from the top. */
  providers: Provider[];
}

export interface RailActive {
  generation: number;
  provider: Provider | null;
}

export interface SummarySelection extends SelectionOrder {
  usage: Usage;
  side: Side;
}

export const getUsage = () => invoke<UsageUpdate>("get_usage");
export const refreshUsage = (provider: Provider | null) =>
  invoke<void>("refresh_usage", { provider });
export const getRailPlacement = () =>
  invoke<RailPlacement>("get_rail_placement");
export const railPointer = (generation: number, provider: Provider | null) =>
  invoke<void>("rail_pointer", { generation, provider });
export const setRailMotion = (generation: number, shifts: RailShifts) =>
  invoke<boolean>("set_rail_motion", { generation, shifts });
export const summaryReady = () => invoke<void>("summary_ready");
export const showSummary = (revision: number, height: number) =>
  invoke<void>("show_summary", { revision, height });

function listen<T>(event: string, handler: (payload: T) => void) {
  return getCurrentWebviewWindow().listen<T>(event, ({ payload }) =>
    handler(payload),
  );
}

export const onRailPlacement = (handler: (payload: RailPlacement) => void) =>
  listen("rail-placement", handler);
export const onRailActive = (handler: (payload: RailActive) => void) =>
  listen("rail-active", handler);
/** Rust found the stored pointer stale: report the indicator again. */
export const onRailPointerReset = (handler: (generation: number) => void) =>
  listen("rail-pointer-reset", handler);
export const onUsageUpdated = (handler: (payload: UsageUpdate) => void) =>
  listen("usage-updated", handler);
export const onSummarySelection = (
  handler: (payload: SummarySelection) => void,
) => listen("summary-selection", handler);
