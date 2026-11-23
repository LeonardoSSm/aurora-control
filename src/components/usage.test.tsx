import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
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

describe("rail", () => {
  it("separates loading and failure from real readings", () => {
    for (const [status, label] of [
      ["loading", "Carregando utilização"],
      ["error", "Falha ao carregar a utilização"],
    ] as const) {
      const rail = renderToStaticMarkup(
        <Rail state={{ status }} onHover={vi.fn()} />,
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
      <Rail
        state={{ status: "ready", snapshot: [sample] }}
        onHover={vi.fn()}
      />,
    );
    expect(rail).toContain("Dados fictícios");
    expect(rail).toContain('stroke-dasharray="64 36"');
    expect(rail).toContain(">64%</span>");
    expect(rail).toContain("Utilização indisponível");
  });

  it("distinguishes zero usage from unavailable data", () => {
    const rail = renderToStaticMarkup(
      <Rail
        state={{ status: "ready", snapshot: [{ ...sample, percentUsed: 0 }] }}
        onHover={vi.fn()}
      />,
    );
    expect(rail).toContain("0% utilizado");
    expect(rail).toContain("Utilização indisponível");
    expect(rail).not.toContain("stroke-dasharray");
  });
});

describe("summary", () => {
  it("labels the fixture and exposes account, interval and time fields", () => {
    const card = renderToStaticMarkup(<UsageSummary usage={sample} />);
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
        usage={{ ...sample, source: "live", status: "stale", percentUsed: 120 }}
      />,
    );
    expect(card).toContain("Dado desatualizado");
    expect(card).toContain("100%");
    expect(card).not.toContain("Dados fictícios");
  });
});
