import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  automaticMetric,
  chooseMetric,
  chooseMonitor,
  enabledCount,
  formatClock,
  metricOptions,
  monitorOptions,
  parseClock,
  primaryValue,
  toggleThreshold,
  updateProvider,
} from "../desktop/settings";
import type {
  Diagnostics,
  Preferences,
  ProviderView,
  SettingsView,
} from "../desktop/settings";
import { catalog } from "../usage/catalog";
import { Settings } from "./Settings";

const now = new Date("2026-11-23T18:00:00Z");

const choice = (enabled: boolean) => ({ enabled, metric: null, alerts: true });

const preferences: Preferences = {
  version: 3,
  side: "right",
  monitor: { id: "m-00000000000000aa", name: "Dell U2720Q" },
  providers: {
    codex: choice(true),
    claudeCode: choice(true),
    antigravity: choice(false),
    grok: choice(false),
  },
  launchAtStartup: false,
  railVisible: true,
  notifications: {
    enabled: true,
    thresholds: [85, 95, 100],
    renewal: true,
    auth: true,
    reconnect: false,
    quiet: { enabled: false, start: 1320, end: 420 },
  },
};

function providerView(id: ProviderView["id"]): ProviderView {
  const entry = catalog.find((candidate) => candidate.id === id)!;
  return {
    id,
    name: entry.name,
    vendor: entry.vendor,
    integration: entry.integration,
    origin: null,
    capabilities: [...entry.capabilities],
    reconnect: entry.reconnect,
    status: "unavailable",
    issue: null,
    account: null,
    plan: null,
    updatedAt: null,
    checkedAt: null,
    metrics: [],
    metricMissing: false,
  };
}

const claude: ProviderView = {
  ...providerView("claudeCode"),
  status: "available",
  account: "d•••@example.com",
  plan: "Max",
  updatedAt: "2026-11-23T17:58:00Z",
  checkedAt: "2026-11-23T17:58:00Z",
  metrics: [
    { id: "fiveHour", label: "Sessão de 5 h", durationMinutes: 300 },
    { id: "sevenDay", label: "Semanal", durationMinutes: 10_080 },
  ],
};

const view: SettingsView = {
  preferences,
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
  placement: { monitor: "m-00000000000000bb", fallback: true, shown: 2 },
  sideOverride: false,
  startup: "disabled",
  startupMismatch: false,
  version: "0.3.0",
  demo: false,
  providers: [
    providerView("codex"),
    claude,
    providerView("antigravity"),
    providerView("grok"),
  ],
  thresholds: [70, 85, 95, 100],
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

function render(
  overrides: Partial<SettingsView> = {},
  extra: Partial<Parameters<typeof Settings>[0]> = {},
) {
  return renderToStaticMarkup(
    <Settings
      view={{ ...view, ...overrides }}
      diagnostics={{ claudeCode: diagnostics }}
      saving={false}
      error={null}
      now={now}
      test="idle"
      onSave={vi.fn()}
      onDiagnose={vi.fn()}
      onTestNotification={vi.fn()}
      {...extra}
    />,
  );
}

describe("settings · sections", () => {
  it("organizes general, placement, providers and notifications", () => {
    const markup = render();
    for (const title of [
      "Geral",
      "Aparência e posicionamento",
      "Provedores",
      "Notificações",
    ]) {
      expect(markup).toContain(`aria-label="${title}"`);
    }
    expect(markup).toContain("Versão instalada");
    expect(markup).toContain("0.3.0");
  });
});

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
    expect(render()).toContain("O monitor escolhido está desconectado");
  });

  it("maps a selection back to a preference", () => {
    expect(chooseMonitor(view, primaryValue)).toBeNull();
    expect(chooseMonitor(view, "m-00000000000000bb")).toEqual({
      id: "m-00000000000000bb",
      name: "Generic PnP Monitor",
    });
    expect(chooseMonitor(view, "m-00000000000000aa")).toEqual(
      view.preferences.monitor,
    );
    expect(chooseMonitor(view, "m-ffffffffffffffff")).toBeNull();
  });

  it("warns when the work area cannot hold every indicator", () => {
    const all = {
      ...preferences,
      providers: {
        codex: choice(true),
        claudeCode: choice(true),
        antigravity: choice(true),
        grok: choice(true),
      },
    };
    expect(
      render({
        preferences: all,
        placement: { monitor: null, fallback: false, shown: 4 },
      }),
    ).not.toContain("comporta");
    const markup = render({
      preferences: all,
      placement: { monitor: null, fallback: false, shown: 3 },
    });
    expect(markup).toContain("comporta 3 de 4 indicadores");
  });
});

describe("settings · providers", () => {
  it("lists every catalog provider with new ones disabled", () => {
    const markup = render();
    for (const entry of catalog) {
      expect(markup).toContain(`aria-label="Mostrar ${entry.name}"`);
    }
    expect(markup).toMatch(
      /aria-checked="false" aria-label="Mostrar Antigravity"/,
    );
    expect(markup).toMatch(/aria-checked="false" aria-label="Mostrar Grok"/);
    // Honest integration levels.
    expect(markup).toContain("Cotas reais pelo CLI oficial");
    expect(markup).toContain("desativada até a verificação dos termos");
  });

  it("allows hiding every provider and explains the hidden rail", () => {
    let none = preferences;
    for (const entry of catalog) {
      none = updateProvider(none, entry.id, { enabled: false });
    }
    expect(enabledCount(none)).toBe(0);
    const markup = render({ preferences: none });
    expect(markup).toContain("Nenhum provedor ativo: a barra fica oculta");
    // No switch is locked.
    expect(markup).not.toMatch(
      /aria-label="Mostrar [^"]+" class="switch" disabled=""/,
    );
  });

  it("shows connection, account and last reading of enabled providers", () => {
    const markup = render();
    expect(markup).toContain("Conectado");
    expect(markup).toContain("d•••@example.com · Max");
    expect(markup).toContain("Última leitura há 2 min");
    expect(markup).toContain("Desativado");
  });

  it("explains a blocked quota source without asking for credentials", () => {
    const grok: ProviderView = {
      ...providerView("grok"),
      issue: { code: "sourceBlocked", reason: "termsUnverified" },
      account: "u•••@example.com",
      checkedAt: "2026-11-23T17:59:00Z",
    };
    const markup = render({
      preferences: updateProvider(preferences, "grok", { enabled: true }),
      providers: [
        providerView("codex"),
        claude,
        providerView("antigravity"),
        grok,
      ],
    });
    expect(markup).toContain("Sessão detectada");
    expect(markup).toContain("termos de uso da xAI");
    // Never a credential value or a prompt for one.
    expect(markup).not.toMatch(/ya29.|Bearer|refresh_token|senha|cookie/i);
  });

  it("names the source of the data", () => {
    const antigravity: ProviderView = {
      ...providerView("antigravity"),
      status: "available",
      origin: "cli",
      updatedAt: "2026-11-23T17:59:00Z",
      checkedAt: "2026-11-23T17:59:00Z",
      metrics: [
        { id: "agy-gemini-5h", label: "Gemini · 5 h", durationMinutes: 300 },
      ],
    };
    const markup = render({
      preferences: updateProvider(preferences, "antigravity", {
        enabled: true,
      }),
      providers: [
        providerView("codex"),
        claude,
        antigravity,
        providerView("grok"),
      ],
    });
    expect(markup).toContain("Fonte: CLI oficial, comando somente leitura");
    expect(markup).toContain("Gemini · 5 h");
  });
});

describe("settings · metric selection", () => {
  it("offers the automatic choice and every window of the reading", () => {
    const options = metricOptions(claude, null);
    expect(options.map((option) => option.value)).toEqual([
      automaticMetric,
      "fiveHour",
      "sevenDay",
    ]);
    expect(chooseMetric(automaticMetric)).toBeNull();
    expect(chooseMetric("sevenDay")).toBe("sevenDay");
  });

  it("keeps a vanished choice listed and offers to go back to automatic", () => {
    const options = metricOptions(claude, "sevenDayOpus");
    expect(options.at(-1)).toEqual({
      value: "sevenDayOpus",
      label: "sevenDayOpus · indisponível",
      missing: true,
    });
    const missing = render({
      preferences: updateProvider(preferences, "claudeCode", {
        metric: "sevenDayOpus",
      }),
      providers: [
        providerView("codex"),
        { ...claude, metricMissing: true },
        providerView("antigravity"),
        providerView("grok"),
      ],
    });
    expect(missing).toContain("Voltar à automática");
    expect(missing).toContain("não apareceu na última leitura");
  });
});

describe("settings · notifications", () => {
  it("toggles thresholds and keeps them sorted", () => {
    expect(toggleThreshold([85, 95], 70)).toEqual([70, 85, 95]);
    expect(toggleThreshold([70, 85, 95], 85)).toEqual([70, 95]);
    const markup = render();
    expect(markup).toMatch(/aria-checked="true" class="threshold">85%/);
    expect(markup).toMatch(/aria-checked="false" class="threshold">70%/);
  });

  it("reads and writes quiet hours as local clock times", () => {
    expect(formatClock(1320)).toBe("22:00");
    expect(formatClock(420)).toBe("07:00");
    expect(parseClock("07:05")).toBe(425);
    expect(parseClock("24:00")).toBeNull();
    expect(parseClock("7h")).toBeNull();
    const quiet = render({
      preferences: {
        ...preferences,
        notifications: {
          ...preferences.notifications,
          quiet: { enabled: true, start: 1320, end: 420 },
        },
      },
    });
    expect(quiet).toContain('value="22:00"');
    expect(quiet).toContain('value="07:00"');
    expect(quiet).toContain("nada fica acumulado");
  });

  it("disables the alert controls when notifications are off", () => {
    const off = render({
      preferences: {
        ...preferences,
        notifications: { ...preferences.notifications, enabled: false },
      },
    });
    expect(off).toMatch(/class="threshold" disabled=""/);
    // The test notification stays available.
    expect(off).toContain("Enviar notificação de teste");
  });

  it("reports the result of a test notification", () => {
    expect(render({}, { test: "sent" })).toContain("Enviada");
    expect(render({}, { test: "failed" })).toContain(
      "O Windows não aceitou a notificação",
    );
  });
});

describe("settings · status and diagnostics", () => {
  it("reports migrated, recovered and newer preference files", () => {
    expect(render({ status: "migrated" })).toContain(
      "Preferências da versão anterior importadas",
    );
    expect(render({ status: "recovered" })).toContain("estava inválido");
    expect(render({ status: "newerVersion" })).toContain("versão mais nova");
    expect(render({ sideOverride: true })).toContain("AURORA_SIDE");
  });

  it("shows diagnostic states without credentials", () => {
    const markup = render();
    expect(markup).toContain("Token vazio");
    expect(markup).toContain('data-state="warning"');
    expect(markup).not.toMatch(/accessToken|refreshToken|sk-ant/i);
  });

  it("describes the start-up entry state", () => {
    expect(render({ startup: "otherLocation" })).toContain("outra cópia");
    expect(render({ startup: "enabled" })).toContain("Ativo");
  });

  it("never presents a registry that disagrees with the preference as in sync", () => {
    expect(render()).not.toContain("Corrigir");
    const stale = render({ startup: "enabled", startupMismatch: true });
    expect(stale).toContain("ainda tem a entrada deste aplicativo");
    expect(stale).toContain("Corrigir");
    const missing = render({
      preferences: { ...view.preferences, launchAtStartup: true },
      startup: "disabled",
      startupMismatch: true,
    });
    expect(missing).toContain("o Windows não tem a entrada");
  });

  it("never diagnoses a disabled provider", () => {
    const markup = render();
    // Only enabled providers have a diagnostics panel.
    expect(markup.match(/class="provider-diagnostics"/g)).toHaveLength(2);
  });
});
