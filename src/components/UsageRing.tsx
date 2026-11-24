import {
  arcPath,
  arcRadius,
  discRadius,
  indicatorDiameter,
  railWidth,
  ringGeometry,
} from "../desktop/railGeometry";
import type { Side } from "../desktop/railGeometry";
import { providerNames } from "../usage/model";
import type { Provider, Usage } from "../usage/model";
import { meter, shortDuration } from "../usage/presentation";

interface UsageRingProps {
  provider: Provider;
  usage: Usage | undefined;
  side: Side;
  shift: number;
  active: boolean;
  onPointer: (provider: Provider | null) => void;
  onRefresh: (provider: Provider) => void;
}

/**
 * Whether the collapsed indicator carries a flag: `issue` when the provider
 * needs the user's attention (sign-in, errors, a chosen metric that is gone),
 * `blocked` when the quota source is unavailable by design.
 */
export function attention(
  usage: Usage | undefined,
): "issue" | "blocked" | null {
  if (!usage || usage.source === "demo") return null;
  if (usage.issue?.code === "sourceBlocked")
    return usage.issue.reason === "cliSafety" ? "issue" : "blocked";
  if (usage.metricMissing) return "issue";
  if (usage.issue && usage.status !== "available") return "issue";
  return null;
}

export function UsageRing({
  provider,
  usage,
  side,
  shift,
  active,
  onPointer,
  onRefresh,
}: UsageRingProps) {
  const reading = meter(usage);
  const { cx, cy, progress, textX } = ringGeometry(side, shift);
  const track = arcPath(side, cx, cy, arcRadius, 1);
  const fill = arcPath(side, cx, cy, arcRadius, reading.percent / 100);
  // Collapsed, only the number fits in the visible half.
  const value =
    progress < 0.5 && reading.text.endsWith("%")
      ? reading.text.slice(0, -1)
      : reading.text;
  // "100" needs a smaller size to fit inside the arc of the visible half.
  const wide = reading.text.length > 3;
  const collapsedSize = wide ? 8.5 : 10.5;
  const expandedSize = wide ? 11.5 : 12.5;
  const tag = shortDuration(usage?.window?.durationMinutes);
  const enter = () => onPointer(provider);
  const flag = attention(usage);
  const gradient = `ring-accent-${provider}`;

  return (
    <div
      className="indicator"
      data-provider={provider}
      data-tone={reading.tone}
      data-flag={flag ?? undefined}
      data-active={active || undefined}
      role="img"
      aria-label={`${providerNames[provider]}, ${reading.label}`}
      onPointerEnter={enter}
      onPointerMove={enter}
      onPointerLeave={() => onPointer(null)}
      onPointerCancel={() => onPointer(null)}
      onClick={() => onRefresh(provider)}
    >
      <svg
        viewBox={`0 0 ${railWidth} ${indicatorDiameter}`}
        width={railWidth}
        height={indicatorDiameter}
        aria-hidden="true"
      >
        <defs>
          {/* Bottom to top, like the gauge: the provider's two accents. */}
          <linearGradient id={gradient} x1="0" y1="1" x2="0" y2="0">
            <stop className="ring-stop-start" offset="0" />
            <stop className="ring-stop-end" offset="1" />
          </linearGradient>
        </defs>
        <circle className="ring-disc" cx={cx} cy={cy} r={discRadius} />
        {track && <path className="ring-track" d={track} />}
        {fill && (
          <path
            className="ring-progress"
            d={fill}
            stroke={`url(#${gradient})`}
          />
        )}
        {flag && (
          <circle
            className="ring-flag"
            cx={textX}
            cy={cy - 15 + 3 * progress}
            r={2.6}
          />
        )}
        <text
          className="ring-value"
          x={textX}
          y={cy + (tag ? -3.5 * progress : 0)}
          fontSize={collapsedSize + (expandedSize - collapsedSize) * progress}
        >
          {value}
        </text>
        {tag && progress > 0.5 && (
          <text
            className="ring-tag"
            x={textX}
            y={cy + 8}
            opacity={Math.max(0, progress * 2 - 1)}
          >
            {tag}
          </text>
        )}
      </svg>
    </div>
  );
}
