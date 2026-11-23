import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { collapsedWidths, expandedWidth } from "../desktop/railGeometry";
import type { Usage } from "../usage/model";
import { Rail } from "./Rail";
import { UsageSummary } from "./UsageSummary";

const sample: Usage = {
  provider: "codex",
  account: "Conta de exemplo",
  source: "demo",
  status: "available",
  percentUsed: 64,
  window: { durationMinutes: 300, resetsAt: "2026-11-23T20:00:00Z" },
  updatedAt: "2026-11-23T17:00:00Z",
};

const railProps = {
  side: "right",
  widths: collapsedWidths,
  onHover: vi.fn(),
} as const;

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
      expect(rail).toContain("Codex");
      expect(rail).toContain("Claude Code");
      expect(rail).not.toContain("stroke-dasharray");
      expect(rail).not.toContain("0%");
    }
  });

  it("marks the demo and draws progress from the same percentage it prints", () => {
    const rail = renderToStaticMarkup(
      <Rail state={{ status: "ready", snapshot: [sample] }} {...railProps} />,
    );
    expect(rail).toContain("Dados fictícios");
    expect(rail).toContain('stroke-dasharray="64 36"');
    expect(rail).toContain(">64%</text>");
    expect(rail).toContain("Utilização indisponível");
  });

  it("distinguishes zero usage from unavailable data", () => {
    const rail = renderToStaticMarkup(
      <Rail
        state={{ status: "ready", snapshot: [{ ...sample, percentUsed: 0 }] }}
        {...railProps}
      />,
    );
    expect(rail).toContain("0% utilizado");
    expect(rail).toContain("Utilização indisponível");
    expect(rail).not.toContain("stroke-dasharray");
  });

  it("anchors the halves on either side and expands only the active provider", () => {
    const state = { status: "ready", snapshot: [sample] } as const;
    const right = renderToStaticMarkup(<Rail state={state} {...railProps} />);
    const left = renderToStaticMarkup(
      <Rail
        state={state}
        {...railProps}
        side="left"
        widths={[expandedWidth, collapsedWidths[1]]}
      />,
    );
    expect(right.match(/cx="64" cy="38" rx="37.5"/g)).toHaveLength(2);
    expect(left).toContain('cx="0" cy="38" rx="63.5"');
    expect(left).toContain('cx="0" cy="38" rx="37.5"');
  });
});

describe("summary", () => {
  it("labels the fixture and exposes account, interval and time fields", () => {
    const card = renderToStaticMarkup(
      <UsageSummary usage={sample} side="right" />,
    );
    for (const value of [
      "Dados fictícios",
      "Conta de exemplo",
      "5 h",
      "Renovação",
      "Atualização",
    ]) {
      expect(card).toContain(value);
    }
  });

  it("shows a stale reading as such and clamps the visual percentage", () => {
    const card = renderToStaticMarkup(
      <UsageSummary
        side="left"
        usage={{ ...sample, source: "live", status: "stale", percentUsed: 120 }}
      />,
    );
    expect(card).toContain("Dado desatualizado");
    expect(card).toContain("100%");
    expect(card).not.toContain("Dados fictícios");
  });
});
