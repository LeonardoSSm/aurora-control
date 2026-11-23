import { providers } from "../usage/model";
import type { RailWidths, Side } from "../desktop/railGeometry";
import type { Provider } from "../usage/model";
import { railStatus } from "../usage/presentation";
import type { RailState } from "../usage/presentation";
import { UsageRing } from "./UsageRing";

export function Rail({
  state,
  side,
  widths,
  onHover,
}: {
  state: RailState;
  side: Side;
  widths: RailWidths;
  onHover: (provider: Provider | null) => void;
}) {
  const badge = railStatus(state);
  const snapshot = state.status === "ready" ? state.snapshot : [];

  return (
    <main
      className="rail select-none"
      aria-label="Aurora Control"
      data-side={side}
    >
      <span className="sr-only" role="status">
        {badge.label}
      </span>
      {providers.map((provider) => (
        <UsageRing
          key={provider}
          provider={provider}
          usage={snapshot.find((usage) => usage.provider === provider)}
          side={side}
          width={provider === "codex" ? widths[0] : widths[1]}
          status={state.status === "ready" ? "" : badge.text}
          onHover={onHover}
        />
      ))}
    </main>
  );
}
