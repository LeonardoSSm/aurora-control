import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  collapsedShifts,
  maxIndicators,
  railHeightFor,
  slide,
} from "../desktop/railGeometry";
import { same, stepShifts, targetShifts } from "../desktop/useRailMotion";
import { catalog, providerNames, providers } from "../usage/catalog";
import type { Provider } from "../usage/catalog";
import type { Usage } from "../usage/model";
import { issueText, meter } from "../usage/presentation";
import { Rail } from "./Rail";
import { attention } from "./UsageRing";
import { UsageSummary } from "./UsageSummary";

const now = new Date("2026-11-24T12:00:00Z");

function usage(provider: Provider, percent: number): Usage {
  return {
    provider,
    account: null,
    plan: null,
    source: "live",
    origin: "api",
    status: "available",
    percentUsed: percent,
    window: {
      id: "w",
      label: "Semanal",
      durationMinutes: 10_080,
      resetsAt: "2026-11-27T00:00:00Z",
      renewed: false,
    },
    quotas: [],
    activity: null,
    updatedAt: "2026-11-24T11:59:00Z",
    checkedAt: "2026-11-24T11:59:00Z",
    issue: null,
    refreshing: false,
    selectedMetric: null,
    metricMissing: false,
  };
}

function rail(visible: readonly Provider[], shifts = collapsedShifts) {
  return renderToStaticMarkup(
    <Rail
      state={{
        status: "ready",
        snapshot: providers.map((provider, index) =>
          usage(provider, 20 + index * 20),
        ),
      }}
      side="right"
      providers={visible}
      shifts={shifts}
      active={null}
      onPointer={vi.fn()}
      onRefresh={vi.fn()}
    />,
  );
}

describe("provider catalog", () => {
  it("lists the four providers in order, shared with Rust", () => {
    expect(providers).toEqual(["codex", "claudeCode", "gemini", "grok"]);
    expect(providerNames).toEqual({
      codex: "Codex",
      claudeCode: "Claude Code",
      gemini: "Gemini",
      grok: "Grok",
    });
    expect(maxIndicators).toBe(catalog.length);
    expect(
      catalog.filter((entry) => entry.defaultEnabled).map((entry) => entry.id),
    ).toEqual(["codex", "claudeCode"]);
  });

  it("gives every provider a visual identity and a slot", () => {
    const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");
    for (const provider of providers) {
      expect(css).toContain(`[data-provider="${provider}"] {`);
    }
    for (let slot = 1; slot <= maxIndicators; slot += 1) {
      expect(css).toContain(`.indicator:nth-of-type(${slot}) {`);
    }
    // Gemini is a two-color gradient; Grok is monochrome silver.
    expect(css).toMatch(/--gemini: #[0-9a-f]{6};\s+--gemini-2: #[0-9a-f]{6};/);
    expect(css).toMatch(/--grok: #[0-9a-f]{6};\s+--grok-2: #[0-9a-f]{6};/);
  });
});

describe("rail with zero to four indicators", () => {
  it("draws exactly the visible providers, in order, without gaps", () => {
    for (let count = 0; count <= maxIndicators; count += 1) {
      const visible = providers.slice(0, count);
      const markup = rail(visible);
      const drawn = [...markup.matchAll(/data-provider="(\w+)"/g)].map(
        (match) => match[1],
      );
      expect(drawn).toEqual(visible);
    }
    // A hidden provider leaves no hole: Codex and Grok are slots 1 and 2.
    const sparse = rail(["codex", "grok"]);
    expect(
      [...sparse.matchAll(/data-provider="(\w+)"/g)].map((match) => match[1]),
    ).toEqual(["codex", "grok"]);
  });

  it("has a height for every count, matching the native layout", () => {
    expect([1, 2, 3, 4].map(railHeightFor)).toEqual([52, 118, 184, 250]);
    expect(railHeightFor(9)).toBe(250);
  });

  it("each provider draws its own gradient", () => {
    const markup = rail(providers);
    for (const provider of providers) {
      expect(markup).toContain(`id="ring-accent-${provider}"`);
      expect(markup).toContain(`stroke="url(#ring-accent-${provider})"`);
    }
  });
});

describe("rail motion with several slots", () => {
  it("expands one slot and collapses the others", () => {
    expect(targetShifts(2)).toEqual([0, 0, slide, 0]);
    expect(targetShifts(-1)).toEqual(collapsedShifts);
    expect(targetShifts(3)).toHaveLength(maxIndicators);
  });

  it("moves every slot toward its target independently", () => {
    const from = [slide, 0, 0, 0];
    const to = targetShifts(3);
    expect(stepShifts(from, to, 0)).toEqual(from);
    expect(stepShifts(from, to, 0.5)).toEqual([16, 0, 0, 16]);
    expect(stepShifts(from, to, 1)).toEqual(to);
    expect(same(stepShifts(from, to, 1), to)).toBe(true);
    expect(same([0, 0], [0, 0, 0, 0])).toBe(false);
  });
});

describe("states of the new providers", () => {
  it("explains blocked sources and incompatible accounts honestly", () => {
    expect(
      issueText(
        { code: "sourceBlocked", reason: "providerTerms" },
        "gemini",
        now,
      ),
    ).toContain("termos do Gemini CLI");
    expect(
      issueText(
        { code: "sourceBlocked", reason: "termsUnverified" },
        "grok",
        now,
      ),
    ).toContain("termos de uso da xAI");
    expect(issueText({ code: "unsupported" }, "gemini", now)).toContain(
      "não são cotas de assinatura",
    );
    expect(issueText({ code: "unsupported" }, "grok", now)).toContain(
      "não são cotas de assinatura",
    );
    expect(issueText({ code: "authMissing" }, "grok", now)).toContain(
      "grok login",
    );
  });

  it("flags attention independently of the provider's colors", () => {
    const blocked: Usage = {
      ...usage("gemini", 0),
      status: "unavailable",
      window: null,
      issue: { code: "sourceBlocked", reason: "providerTerms" },
    };
    expect(attention(blocked)).toBe("blocked");
    const expired: Usage = {
      ...usage("grok", 40),
      status: "stale",
      percentUsed: 40,
      issue: { code: "authExpired" },
    };
    expect(attention(expired)).toBe("issue");
    expect(attention(usage("grok", 40))).toBeNull();
    expect(attention({ ...usage("grok", 40), source: "demo" })).toBeNull();
    const markup = renderToStaticMarkup(
      <Rail
        state={{ status: "ready", snapshot: [blocked] }}
        side="left"
        providers={["gemini"]}
        shifts={collapsedShifts}
        active={null}
        onPointer={vi.fn()}
        onRefresh={vi.fn()}
      />,
    );
    expect(markup).toContain('data-flag="blocked"');
    expect(markup).toContain('class="ring-flag"');
  });

  it("never switches metric silently when the chosen one is missing", () => {
    const missing: Usage = {
      ...usage("claudeCode", 0),
      status: "unavailable",
      window: null,
      selectedMetric: "sevenDayOpus",
      metricMissing: true,
    };
    expect(meter(missing)).toMatchObject({ text: "—", tone: "none" });
    expect(attention(missing)).toBe("issue");
    const summary = renderToStaticMarkup(
      <UsageSummary usage={missing} side="right" now={now} />,
    );
    expect(summary).toContain("Métrica escolhida indisponível");
    expect(summary).toContain("seleção automática");
  });
});
