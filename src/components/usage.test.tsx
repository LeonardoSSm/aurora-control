import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  arcPath,
  arcPoint,
  arcRadius,
  collapsedShifts,
  indicatorRadius,
  railHeight,
  railWidth,
  ringGeometry,
  slide,
} from "../desktop/railGeometry";
import type { Usage } from "../usage/model";
import { formatReset, formatSpan, meter } from "../usage/presentation";
import { Rail } from "./Rail";
import { UsageSummary } from "./UsageSummary";

const now = new Date("2026-11-23T18:00:00Z");

const sample: Usage = {
  provider: "codex",
  account: "d•••@example.com",
  plan: "Plus",
  source: "live",
  origin: "api",
  status: "available",
  percentUsed: 73,
  window: {
    id: "codex-secondary",
    label: "Semanal",
    durationMinutes: 10_080,
    resetsAt: "2026-11-26T08:29:17Z",
  },
  quotas: [
    {
      id: "codex-primary",
      label: "Janela de 5 h",
      kind: "window",
      percentUsed: 0,
      durationMinutes: 300,
      resetsAt: "2026-11-23T23:05:25Z",
    },
  ],
  activity: null,
  updatedAt: "2026-11-23T17:58:00Z",
  checkedAt: "2026-11-23T17:58:00Z",
  issue: null,
  refreshing: false,
};

type RailProps = Parameters<typeof Rail>[0];

const railProps: Omit<RailProps, "state"> = {
  side: "right",
  shifts: collapsedShifts,
  active: null,
  onPointer: vi.fn(),
  onRefresh: vi.fn(),
};

function render(usage: Usage, props: Partial<RailProps> = {}) {
  return renderToStaticMarkup(
    <Rail
      state={{ status: "ready", snapshot: [usage] }}
      {...railProps}
      {...props}
    />,
  );
}

const close = (a: number, b: number) => expect(a).toBeCloseTo(b, 6);

describe("semicircular gauge geometry", () => {
  it("shares the native dimensions", () => {
    expect([slide, railWidth, railHeight]).toEqual([32, 60, 118]);
  });

  it("maps each percentage to the matching share of the visible half", () => {
    const cx = railWidth;
    const cy = indicatorRadius;
    const r = arcRadius;
    // Right side: bottom (0%) → innermost point (50%) → top (100%).
    const expected = [
      [0, cx, cy + r],
      [0.25, cx - r * Math.SQRT1_2, cy + r * Math.SQRT1_2],
      [0.5, cx - r, cy],
      [0.75, cx - r * Math.SQRT1_2, cy - r * Math.SQRT1_2],
      [1, cx, cy - r],
    ] as const;
    for (const [fraction, x, y] of expected) {
      const point = arcPoint("right", cx, cy, r, fraction);
      close(point.x, x);
      close(point.y, y);
    }
  });

  it("ends the progress path exactly at the fraction and draws nothing at 0%", () => {
    expect(arcPath("right", 60, 26, 20, 0)).toBeNull();
    for (const fraction of [0.1, 0.25, 0.5, 0.75, 1]) {
      const path = arcPath("right", 60, 26, 20, fraction)!;
      const end = arcPoint("right", 60, 26, 20, fraction);
      const [x, y] = path.split(" ").slice(-2).map(Number);
      close(x!, Number(end.x.toFixed(2)));
      close(y!, Number(end.y.toFixed(2)));
      // No single segment spans more than a quarter turn of the semicircle.
      expect(path.match(/A /g)).toHaveLength(fraction > 0.5 ? 2 : 1);
    }
  });

  it("mirrors the gauge between the sides", () => {
    for (const fraction of [0, 0.3, 0.5, 0.9, 1]) {
      const right = arcPoint("right", railWidth, 26, arcRadius, fraction);
      const left = arcPoint("left", 0, 26, arcRadius, fraction);
      close(railWidth - right.x, left.x);
      close(right.y, left.y);
    }
    expect(arcPath("right", 60, 26, 20, 1)).toContain("0 0 1");
    expect(arcPath("left", 0, 26, 20, 1)).toContain("0 0 0");
  });

  it("centers the collapsed circle on the edge and slides it fully inside", () => {
    expect(ringGeometry("right", 0).cx).toBe(railWidth);
    expect(ringGeometry("left", 0).cx).toBe(0);
    expect(ringGeometry("right", slide).cx).toBe(railWidth - slide);
    expect(ringGeometry("left", slide).cx).toBe(slide);
    // Fully visible: 6 px between the circle and the screen edge.
    expect(
      railWidth - (ringGeometry("right", slide).cx + indicatorRadius),
    ).toBe(6);
    expect(ringGeometry("left", slide).cx - indicatorRadius).toBe(6);
    // Out-of-range offsets are clamped.
    expect(ringGeometry("right", 99).cx).toBe(railWidth - slide);
  });
});

describe("rail", () => {
  it("separates loading and failure from real readings", () => {
    for (const [status, label] of [
      ["loading", "Carregando utilização"],
      ["error", "Falha ao carregar a utilização"],
    ] as const) {
      const rail = renderToStaticMarkup(
        <Rail state={{ status }} {...railProps} />,
      );
      expect(rail).toContain(label);
      expect(rail).toContain("Codex, Utilização indisponível");
      expect(rail).toContain("Claude Code, Utilização indisponível");
      expect(rail).not.toContain("ring-progress");
    }
  });

  it("draws the progress from the same percentage it announces", () => {
    const rail = render(sample);
    const geometry = ringGeometry("right", 0);
    const path = arcPath("right", geometry.cx, geometry.cy, arcRadius, 0.73)!;
    expect(rail).toContain(`class="ring-progress" d="${path}"`);
    expect(rail).toContain("Codex, 73% utilizado, semanal");
    // Collapsed: only the number fits in the visible half.
    expect(rail).toContain(">73</text>");
    expect(rail).not.toContain("ring-tag");
  });

  it("distinguishes zero usage from unavailable data", () => {
    const rail = render({ ...sample, percentUsed: 0 });
    expect(rail).toContain("0% utilizado");
    expect(rail).toContain("Claude Code, Utilização indisponível");
    expect(rail).not.toContain("ring-progress");
    expect(rail.match(/ring-track/g)).toHaveLength(2);
  });

  it("expands only the active indicator and reveals its window tag", () => {
    const rail = render(sample, {
      side: "left",
      shifts: [slide, 0],
      active: "codex",
    });
    expect(rail).toContain(`cx="${slide}"`);
    expect(rail).toContain('cx="0"');
    expect(rail).toContain('data-active="true"');
    expect(rail).toContain(">73%</text>");
    expect(rail).toContain(">7 d</text>");
  });

  it("marks stale and critical readings with distinct tones", () => {
    expect(render({ ...sample, status: "stale" })).toContain(
      'data-tone="stale"',
    );
    expect(render({ ...sample, percentUsed: 95 })).toContain(
      'data-tone="critical"',
    );
    expect(meter({ ...sample, status: "stale" }).label).toContain(
      "dado desatualizado",
    );
  });
});

describe("summary", () => {
  it("shows provider, account, primary window, renewal and other quotas", () => {
    const card = renderToStaticMarkup(
      <UsageSummary usage={sample} side="right" now={now} />,
    );
    for (const value of [
      "Codex",
      "Plus",
      "d•••@example.com",
      "73%",
      "Semanal",
      "Renova em 2 d 14 h",
      "Janela de 5 h",
      "Renova em 5 h 5 min · 23:05",
      "Conectado · atualizado há 2 min",
    ]) {
      expect(card).toContain(value);
    }
    expect(card).not.toContain("Dados fictícios");
    expect(card).not.toContain('role="alert"');
  });

  it("explains stale data from the session log and the cause", () => {
    const card = renderToStaticMarkup(
      <UsageSummary
        side="left"
        now={now}
        usage={{
          ...sample,
          status: "stale",
          origin: "sessionLog",
          updatedAt: "2026-11-23T17:16:00Z",
          issue: { code: "authExpired" },
        }}
      />,
    );
    expect(card).toContain("Desatualizado · registro local há 44 min");
    expect(card).toContain("A sessão do Codex expirou");
    expect(card).toContain('data-connection="stale"');
  });

  it("is honest when nothing is available", () => {
    const card = renderToStaticMarkup(
      <UsageSummary
        side="right"
        now={now}
        usage={{
          ...sample,
          provider: "claudeCode",
          status: "unavailable",
          window: null,
          quotas: [],
          updatedAt: null,
          issue: { code: "rateLimited", retryAt: "2026-11-23T18:10:00Z" },
          activity: { windowMinutes: 300, tokens: 184_250, sessions: 2 },
        }}
      />,
    );
    expect(card).toContain("—");
    expect(card).toContain("Consumo indisponível");
    expect(card).toContain("Indisponível");
    expect(card).toContain("Limite de consultas atingido");
    expect(card).toContain("Sessões locais · 5 h");
    expect(card).toContain("2 sessões");
    expect(card).not.toContain("Renova em");
  });

  it("labels fixtures", () => {
    const card = renderToStaticMarkup(
      <UsageSummary
        side="right"
        now={now}
        usage={{ ...sample, source: "demo", origin: "fixture" }}
      />,
    );
    expect(card).toContain("Dados fictícios");
    expect(card).toContain('data-connection="demo"');
  });
});

describe("time formatting", () => {
  it("formats spans and renewals", () => {
    expect(formatSpan(30_000)).toBe("menos de 1 min");
    expect(formatSpan(134 * 60_000)).toBe("2 h 14 min");
    expect(formatSpan(3 * 86_400_000)).toBe("3 d");
    expect(formatReset("2026-11-23T17:00:00Z", now)).toBe("Renovada");
    expect(formatReset(null, now)).toBe("Renovação desconhecida");
  });
});
