import {
  collapsedWidth,
  expandedWidth,
  indicatorDiameter,
  ringGeometry,
} from "../desktop/railGeometry";
import type { Side } from "../desktop/railGeometry";
import { providerNames } from "../usage/model";
import type { Provider, Usage } from "../usage/model";
import { meter } from "../usage/presentation";

interface UsageRingProps {
  provider: Provider;
  usage: Usage | undefined;
  side: Side;
  width: number;
  status: string;
  onHover: (provider: Provider | null) => void;
}

export function UsageRing({
  provider,
  usage,
  side,
  width,
  status,
  onHover,
}: UsageRingProps) {
  const reading = meter(usage);
  const name = providerNames[provider];
  const label = provider === "claudeCode" ? "Claude" : name;
  const badge = usage?.source === "demo" ? "Demo" : status;
  const { edge, centerY, arc, textX } = ringGeometry(side, width);
  const growth = width - collapsedWidth;
  const surface = `${provider}-surface`;
  const enter = () => {
    if (usage) onHover(provider);
  };

  return (
    <div
      className="indicator"
      data-provider={provider}
      role="img"
      aria-label={`${name}, ${reading.label}`}
      onPointerEnter={enter}
      onPointerMove={enter}
      onPointerLeave={() => onHover(null)}
      onPointerCancel={() => onHover(null)}
    >
      <svg
        viewBox={`0 0 ${expandedWidth} ${indicatorDiameter}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <defs>
          <radialGradient
            id={surface}
            gradientUnits="userSpaceOnUse"
            cx={textX}
            cy="21"
            r={width + 12}
          >
            <stop className="ring-glow" />
            <stop offset="1" className="ring-base" />
          </radialGradient>
        </defs>
        <ellipse
          className="ring-surface"
          cx={edge}
          cy={centerY}
          rx={width - 0.5}
          ry={centerY - 0.5}
          fill={`url(#${surface})`}
        />
        <path className="ring-track" d={arc} />
        {reading.percent > 0 && (
          <path
            className="ring-progress"
            d={arc}
            pathLength="100"
            strokeDasharray={`${reading.percent} ${100 - reading.percent}`}
          />
        )}
        {badge && (
          <text className="ring-badge" x={textX} y="24">
            {badge}
          </text>
        )}
        <text
          className="ring-value"
          x={textX}
          y="40"
          fontSize={13 + growth * 0.24}
          data-status={usage?.status}
        >
          {reading.text}
        </text>
        <text
          className="provider-name"
          x={textX}
          y="55"
          fontSize={8 + growth * 0.075}
        >
          {label}
        </text>
      </svg>
    </div>
  );
}
