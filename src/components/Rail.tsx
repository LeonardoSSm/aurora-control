import { providers } from "../usage/model";
import type { Provider } from "../usage/model";
import { railStatus } from "../usage/presentation";
import type { RailState } from "../usage/presentation";
import { UsageRing } from "./UsageRing";

export function Rail({
  state,
  onHover,
}: {
  state: RailState;
  onHover: (provider: Provider | null) => void;
}) {
  const badge = railStatus(state);
  const snapshot = state.status === "ready" ? state.snapshot : [];

  return (
    <main className="rail h-screen select-none" aria-label="Aurora Control">
      <div
        className="rail-header flex items-center justify-center"
        role="status"
        aria-label={badge.label}
      >
        {badge.text && <span className="rail-badge">{badge.text}</span>}
      </div>
      {providers.map((provider) => (
        <UsageRing
          key={provider}
          provider={provider}
          usage={snapshot.find((usage) => usage.provider === provider)}
          onHover={onHover}
        />
      ))}
    </main>
  );
}
