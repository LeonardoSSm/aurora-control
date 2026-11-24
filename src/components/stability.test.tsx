import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { isNewerSelection, isNewerUpdate } from "../desktop/ordering";
import type { Usage } from "../usage/model";
import { issueText, meter } from "../usage/presentation";
import { UsageSummary } from "./UsageSummary";

const now = new Date("2026-11-23T18:00:00Z");

const usage: Usage = {
  provider: "claudeCode",
  account: "d•••@example.com",
  plan: "Max",
  source: "live",
  origin: "api",
  status: "available",
  percentUsed: 91,
  window: {
    id: "fiveHour",
    label: "Sessão de 5 h",
    durationMinutes: 300,
    resetsAt: "2026-11-23T18:01:00Z",
    renewed: false,
  },
  quotas: [
    {
      id: "sevenDay",
      label: "Semanal · todos os modelos",
      kind: "window",
      percentUsed: 40,
      durationMinutes: 10_080,
      resetsAt: "2026-11-23T17:59:00Z",
      renewed: false,
    },
  ],
  activity: { windowMinutes: 300, tokens: 1_000, sessions: 1, partial: true },
  updatedAt: "2026-11-23T17:55:00Z",
  checkedAt: "2026-11-23T17:55:00Z",
  issue: null,
  refreshing: false,
  selectedMetric: null,
  metricMissing: false,
};

describe("I03 · out-of-order delivery", () => {
  it("drops an older snapshot delivered after a newer one", () => {
    let shown: number | null = null;
    const delivered: number[] = [];
    for (const sequence of [4, 2, 5, 5, 3]) {
      if (isNewerUpdate(shown, sequence)) {
        shown = sequence;
        delivered.push(sequence);
      }
    }
    expect(delivered).toEqual([4, 5]);
  });

  it("orders selections by revision, then by data sequence", () => {
    const first = { revision: 1, sequence: 10 };
    const second = { revision: 2, sequence: 9 };
    // A new selection wins even with an older data sequence.
    expect(isNewerSelection(first, second)).toBe(true);
    // The old selection arriving late is dropped.
    expect(isNewerSelection(second, first)).toBe(false);
    // Two updates of the same selection, delivered inverted.
    expect(isNewerSelection(second, { revision: 2, sequence: 12 })).toBe(true);
    expect(
      isNewerSelection(
        { revision: 2, sequence: 12 },
        { revision: 2, sequence: 11 },
      ),
    ).toBe(false);
    expect(isNewerSelection(null, first)).toBe(true);
  });
});

describe("I02 · renewal without a new measurement", () => {
  it("never shows a renewed window as a fresh percentage", () => {
    const renewed: Usage = {
      ...usage,
      status: "stale",
      percentUsed: 0,
      window: { ...usage.window!, resetsAt: null, renewed: true },
    };
    const reading = meter(renewed);
    expect(reading.text).toBe("—");
    expect(reading.tone).toBe("stale");
    expect(reading.label).toContain("aguardando nova leitura");
  });

  it("re-evaluates renewals in an open card as time passes", () => {
    // Before the renewal, the primary shows its reading.
    expect(meter(usage, now).text).toBe("91%");
    const later = new Date("2026-11-23T18:01:30Z");
    expect(meter(usage, later).text).toBe("—");
    const card = renderToStaticMarkup(
      <UsageSummary usage={usage} side="right" now={later} />,
    );
    // The weekly quota already renewed at 17:59: no stale 40%.
    expect(card).not.toContain("40%");
    expect(card).toContain("Renovada · aguardando nova leitura");
    expect(card).toContain("parcial");
  });
});

describe("I06 · credential diagnostics", () => {
  it("explains credentials the app cannot read without calling it a logout", () => {
    const claude = issueText(
      { code: "credentialsUnavailable" },
      "claudeCode",
      now,
    );
    expect(claude).toContain("Claude Desktop");
    expect(claude).not.toContain("Nenhuma sessão");
    expect(
      issueText({ code: "credentialsUnavailable" }, "codex", now),
    ).toContain("armazenamento do sistema");
    expect(issueText({ code: "credentialsInvalid" }, "codex", now)).toContain(
      "não pôde ser lido",
    );
    expect(
      issueText(
        { code: "http", status: 503, retryAt: "2026-11-23T18:05:00Z" },
        "codex",
        now,
      ),
    ).toContain("Nova tentativa");
  });
});
