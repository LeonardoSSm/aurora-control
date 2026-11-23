import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  chooseMonitor,
  monitorOptions,
  primaryValue,
  toggleProvider,
} from "../desktop/settings";
import type { Diagnostics, SettingsView } from "../desktop/settings";
import { Settings } from "./Settings";

const now = new Date("2026-11-23T18:00:00Z");

const view: SettingsView = {
  preferences: {
    version: 1,
    side: "right",
    monitor: { id: "m-00000000000000aa", name: "Dell U2720Q" },
    providers: { codex: true, claudeCode: true },
    launchAtStartup: false,
  },
  status: "loaded",
  monitors: [
    {
      id: "m-00000000000000bb",
      name: "Generic PnP Monitor",
      primary: true,
      width: 1920,
      height: 1080,
      scale: 1.25,
    },
  ],
  placement: { monitor: "m-00000000000000bb", fallback: true },
  sideOverride: false,
  startup: "disabled",
  version: "0.2.0",
  demo: false,
};

const diagnostics: Diagnostics = {
  provider: "claudeCode",
  rows: [
    {
      label: "Credencial do CLI (.credentials.json)",
      state: "warning",
      detail: "Token vazio",
      at: null,
    },
    {
      label: "Validade",
      state: "warning",
      detail: "Expirado",
      at: "2026-10-29T12:00:00Z",
    },
  ],
  issue: { code: "credentialsUnavailable" },
  checkedAt: "2026-11-23T17:59:00Z",
};

function render(overrides: Partial<SettingsView> = {}) {
  return renderToStaticMarkup(
    <Settings
      view={{ ...view, ...overrides }}
      diagnostics={diagnostics}
      saving={false}
      error={null}
      now={now}
      onSave={vi.fn()}
      onRecheck={vi.fn()}
    />,
  );
}

describe("settings · monitors", () => {
  it("keeps a disconnected monitor listed and explains the fallback", () => {
    const options = monitorOptions(view);
    expect(options.map((option) => option.value)).toEqual([
      primaryValue,
      "m-00000000000000bb",
      "m-00000000000000aa",
    ]);
    expect(options[1]?.label).toBe(
      "Generic PnP Monitor · 1920 × 1080 · 125% · principal",
    );
    expect(options[2]).toMatchObject({
      disconnected: true,
      label: "Dell U2720Q · desconectado",
    });
    const markup = render();
    expect(markup).toContain("O monitor escolhido está desconectado");
  });

  it("maps a selection back to a preference", () => {
    expect(chooseMonitor(view, primaryValue)).toBeNull();
    expect(chooseMonitor(view, "m-00000000000000bb")).toEqual({
      id: "m-00000000000000bb",
      name: "Generic PnP Monitor",
    });
    // The disconnected choice is kept as it was.
    expect(chooseMonitor(view, "m-00000000000000aa")).toEqual(
      view.preferences.monitor,
    );
    // Unknown values from a stale list fall back to the primary.
    expect(chooseMonitor(view, "m-ffffffffffffffff")).toBeNull();
  });
});

describe("settings · providers", () => {
  it("never hides the last visible provider", () => {
    const onlyClaude = toggleProvider(view.preferences, "codex")!;
    expect(onlyClaude.providers).toEqual({ codex: false, claudeCode: true });
    expect(toggleProvider(onlyClaude, "claudeCode")).toBeNull();
    const markup = render({ preferences: onlyClaude });
    // The remaining switch is disabled.
    expect(markup).toMatch(
      /aria-checked="true" aria-label="Mostrar Claude Code" class="switch" disabled=""/,
    );
  });
});

describe("settings · status and diagnostics", () => {
  it("reports recovered and newer preference files", () => {
    expect(render({ status: "recovered" })).toContain("estava inválido");
    expect(render({ status: "newerVersion" })).toContain("versão mais nova");
    expect(render({ sideOverride: true })).toContain("AURORA_SIDE");
  });

  it("shows diagnostic states without credentials", () => {
    const markup = render();
    expect(markup).toContain("Token vazio");
    expect(markup).toContain('data-state="warning"');
    expect(markup).toContain("Claude Desktop");
    expect(markup).not.toMatch(/accessToken|refreshToken|sk-ant/i);
  });

  it("describes the start-up entry state", () => {
    expect(render({ startup: "otherLocation" })).toContain("outra cópia");
    expect(render({ startup: "enabled" })).toContain("Ativo");
  });
});

describe("settings · hidden provider", () => {
  it("explains that a hidden Claude Code is not diagnosed", () => {
    const markup = renderToStaticMarkup(
      <Settings
        view={{
          ...view,
          preferences: {
            ...view.preferences,
            providers: { codex: true, claudeCode: false },
          },
        }}
        diagnostics={null}
        saving={false}
        error={null}
        now={now}
        onSave={vi.fn()}
        onRecheck={vi.fn()}
      />,
    );
    expect(markup).toContain("Claude Code oculto: nenhum arquivo dele é lido");
    expect(markup).not.toContain("Token vazio");
  });
});
