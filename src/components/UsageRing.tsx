import { providerNames } from "../usage/model";
import type { Provider, Usage } from "../usage/model";
import { meter } from "../usage/presentation";

interface UsageRingProps {
  provider: Provider;
  usage: Usage | undefined;
  onHover: (provider: Provider | null) => void;
}

export function UsageRing({ provider, usage, onHover }: UsageRingProps) {
  const reading = meter(usage);
  const name = providerNames[provider];

  return (
    <div
      className="indicator flex flex-col items-center"
      data-provider={provider}
      onPointerEnter={() => usage && onHover(provider)}
      onPointerLeave={() => usage && onHover(null)}
      onPointerCancel={() => usage && onHover(null)}
    >
      <div className="usage-ring relative grid place-items-center">
        <svg
          className="absolute inset-0 size-full"
          viewBox="0 0 60 60"
          aria-hidden="true"
        >
          <circle className="ring-track" cx="30" cy="30" r="26" />
          {reading.percent > 0 && (
            <circle
              className="ring-progress"
              cx="30"
              cy="30"
              r="26"
              pathLength="100"
              strokeDasharray={`${reading.percent} ${100 - reading.percent}`}
              transform="rotate(-90 30 30)"
            />
          )}
        </svg>
        <span
          className="ring-value tabular-nums"
          role="img"
          aria-label={reading.label}
          data-status={usage?.status}
        >
          {reading.text}
        </span>
      </div>
      <span className="provider-name text-center">{name}</span>
    </div>
  );
}
