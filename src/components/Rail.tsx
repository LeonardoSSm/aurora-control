import type { RailShifts, Side } from "../desktop/railGeometry";
import type { Provider } from "../usage/model";
import { railStatus } from "../usage/presentation";
import type { RailState } from "../usage/presentation";
import { UsageRing } from "./UsageRing";

export function Rail({
  state,
  side,
  providers,
  shifts,
  active,
  onPointer,
  onRefresh,
}: {
  state: RailState;
  side: Side;
  /** Visible indicators, from the top: one slot each. */
  providers: readonly Provider[];
  shifts: RailShifts;
  active: Provider | null;
  onPointer: (provider: Provider | null) => void;
  onRefresh: (provider: Provider) => void;
}) {
  const snapshot = state.status === "ready" ? state.snapshot : [];

  return (
    <main
      className="rail select-none"
      aria-label="Aurora Control"
      data-side={side}
    >
      <span className="sr-only" role="status">
        {railStatus(state).label}
      </span>
      {providers.map((provider, index) => (
        <UsageRing
          key={provider}
          provider={provider}
          usage={snapshot.find((usage) => usage.provider === provider)}
          side={side}
          shift={shifts[index] ?? 0}
          active={active === provider}
          onPointer={onPointer}
          onRefresh={onRefresh}
        />
      ))}
    </main>
  );
}
