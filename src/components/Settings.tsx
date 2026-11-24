import type { ReactNode } from "react";
import {
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
  automaticMetric,
} from "../desktop/settings";
import type {
  Diagnostics,
  LoadStatus,
  Preferences,
  ProviderView,
  SettingsView,
  StartupStatus,
} from "../desktop/settings";
import { formatAgo, formatMoment, issueText } from "../usage/presentation";

const loadNotes: Partial<Record<LoadStatus, string>> = {
  migrated:
    "Preferências da versão anterior importadas. O arquivo será gravado no formato novo na próxima alteração.",
  recovered:
    "O arquivo de preferências estava inválido. Ele foi preservado ao lado e os padrões estão em uso.",
  newerVersion:
    "As preferências foram gravadas por uma versão mais nova. Elas só serão substituídas se você alterar algo aqui.",
  unavailable:
    "Pasta de configuração indisponível: as escolhas valem só até fechar o aplicativo.",
};

const startupNotes: Record<StartupStatus, string> = {
  enabled: "Ativo para este usuário do Windows.",
  disabled: "Desativado.",
  otherLocation:
    "Existe uma entrada de outra cópia do aplicativo. Ativar aqui a substitui.",
  unknown: "Não foi possível ler o registro do Windows.",
};

const integrationNotes: Record<ProviderView["integration"], string> = {
  live: "Cotas reais da conta",
  authOnly: "Somente sessão local · sem fonte de cotas autorizada",
  gated:
    "Somente sessão local · consulta desativada até a verificação dos termos",
};

export type TestState = "idle" | "sending" | "sent" | "failed";

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <section className="settings-section" aria-label={title}>
      <header>
        <h2>{title}</h2>
        {hint && <p>{hint}</p>}
      </header>
      <div className="settings-card">{children}</div>
    </section>
  );
}

function Switch({
  checked,
  disabled,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: () => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className="switch"
      disabled={disabled}
      onClick={onChange}
    >
      <span className="switch-thumb" />
    </button>
  );
}

function statusChip(provider: ProviderView, enabled: boolean, demo: boolean) {
  if (!enabled) return { label: "Desativado", state: "off" };
  if (demo) return { label: "Demonstração", state: "demo" };
  if (provider.issue?.code === "sourceBlocked")
    return { label: "Sessão detectada", state: "info" };
  switch (provider.status) {
    case "available":
      return { label: "Conectado", state: "ok" };
    case "stale":
      return { label: "Desatualizado", state: "warning" };
    case "unavailable":
      return provider.checkedAt
        ? { label: "Indisponível", state: "warning" }
        : { label: "Verificando…", state: "info" };
  }
}

function DiagnosticList({
  diagnostics,
  now,
}: {
  diagnostics: Diagnostics;
  now: Date;
}) {
  return (
    <ul className="diagnostics">
      {diagnostics.rows.map((row, index) => (
        <li key={`${row.label}-${index}`} data-state={row.state}>
          <span className="diagnostic-dot" aria-hidden="true" />
          <span className="diagnostic-label">{row.label}</span>
          <span className="diagnostic-detail">
            {row.detail}
            {row.at && ` · ${formatMoment(row.at, now)}`}
          </span>
        </li>
      ))}
    </ul>
  );
}

function ProviderCard({
  provider,
  preferences,
  demo,
  saving,
  diagnostics,
  now,
  onSave,
  onDiagnose,
}: {
  provider: ProviderView;
  preferences: Preferences;
  demo: boolean;
  saving: boolean;
  diagnostics: Diagnostics | null | undefined;
  now: Date;
  onSave: (preferences: Preferences) => void;
  onDiagnose: () => void;
}) {
  const choice = preferences.providers[provider.id];
  const chip = statusChip(provider, choice.enabled, demo);
  const options = metricOptions(provider, choice.metric);
  const updated = formatAgo(provider.updatedAt, now);
  const checked = formatAgo(provider.checkedAt, now);

  return (
    <article
      className="provider-card"
      data-provider={provider.id}
      data-enabled={choice.enabled || undefined}
      aria-label={provider.name}
    >
      <header className="provider-card-head">
        <span className="provider-mark" aria-hidden="true" />
        <div className="provider-card-title">
          <h3>{provider.name}</h3>
          <p>{provider.vendor}</p>
        </div>
        <span className="status-chip" data-state={chip.state}>
          {chip.label}
        </span>
        <Switch
          checked={choice.enabled}
          disabled={saving}
          label={`Mostrar ${provider.name}`}
          onChange={() =>
            onSave(
              updateProvider(preferences, provider.id, {
                enabled: !choice.enabled,
              }),
            )
          }
        />
      </header>

      <p className="provider-integration">
        {integrationNotes[provider.integration]}
      </p>

      {choice.enabled && !demo && (
        <>
          {(provider.account || provider.plan) && (
            <p className="provider-line">
              {[provider.account, provider.plan].filter(Boolean).join(" · ")}
            </p>
          )}
          {provider.issue && (
            <p className="provider-issue">
              {issueText(provider.issue, provider.id, now)}
            </p>
          )}
          <p className="provider-line provider-times">
            {updated
              ? `Última leitura ${updated}`
              : "Nenhuma leitura de cotas ainda"}
            {checked && ` · verificado ${checked}`}
          </p>
        </>
      )}

      {choice.enabled && (
        <div className="settings-row">
          <label htmlFor={`metric-${provider.id}`}>Métrica do indicador</label>
          <select
            id={`metric-${provider.id}`}
            value={choice.metric ?? automaticMetric}
            disabled={saving}
            onChange={(event) =>
              onSave(
                updateProvider(preferences, provider.id, {
                  metric: chooseMetric(event.target.value),
                }),
              )
            }
          >
            {options.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>
      )}
      {choice.enabled && provider.metricMissing && (
        <div className="settings-hint settings-warning" role="alert">
          <p>
            A métrica escolhida não apareceu na última leitura. O indicador
            mostra “—” em vez de trocar de métrica.
          </p>
          <div className="settings-actions">
            <button
              type="button"
              className="ghost"
              disabled={saving}
              onClick={() =>
                onSave(
                  updateProvider(preferences, provider.id, { metric: null }),
                )
              }
            >
              Voltar à automática
            </button>
          </div>
        </div>
      )}

      {choice.enabled && !demo && (
        <details
          className="provider-diagnostics"
          onToggle={(event) => {
            if (event.currentTarget.open) onDiagnose();
          }}
        >
          <summary>Diagnóstico</summary>
          {diagnostics ? (
            <DiagnosticList diagnostics={diagnostics} now={now} />
          ) : (
            <p className="settings-hint">
              {diagnostics === null
                ? "Diagnóstico indisponível."
                : "Lendo a sessão local…"}
            </p>
          )}
          <p className="settings-hint">
            Somente leitura. Nenhum token, conteúdo de arquivo ou identificador
            interno é exibido. Para reconectar: {provider.reconnect}
          </p>
          <div className="settings-actions">
            <button type="button" className="ghost" onClick={onDiagnose}>
              Verificar novamente
            </button>
          </div>
        </details>
      )}
    </article>
  );
}

function ClockInput({
  id,
  label,
  value,
  disabled,
  onChange,
}: {
  id: string;
  label: string;
  value: number;
  disabled: boolean;
  onChange: (minutes: number) => void;
}) {
  return (
    <label className="clock" htmlFor={id}>
      <span>{label}</span>
      <input
        id={id}
        type="time"
        value={formatClock(value)}
        disabled={disabled}
        onChange={(event) => {
          const minutes = parseClock(event.target.value);
          if (minutes !== null) onChange(minutes);
        }}
      />
    </label>
  );
}

export function Settings({
  view,
  diagnostics,
  saving,
  error,
  now,
  test,
  onSave,
  onDiagnose,
  onTestNotification,
}: {
  view: SettingsView;
  diagnostics: Partial<Record<ProviderView["id"], Diagnostics | null>>;
  saving: boolean;
  error: string | null;
  now: Date;
  test: TestState;
  onSave: (preferences: Preferences) => void;
  onDiagnose: (provider: ProviderView["id"]) => void;
  onTestNotification: () => void;
}) {
  const preferences = view.preferences;
  const options = monitorOptions(view);
  const selected = preferences.monitor?.id ?? primaryValue;
  const enabled = enabledCount(preferences);
  const notifications = preferences.notifications;
  const saveNotifications = (change: Partial<typeof notifications>) =>
    onSave({
      ...preferences,
      notifications: { ...notifications, ...change },
    });
  const quiet = notifications.quiet;
  const alertsOff = saving || !notifications.enabled;

  return (
    <main className="settings select-none">
      <header className="settings-header">
        <span className="settings-mark" aria-hidden="true" />
        <div>
          <h1>Aurora Control</h1>
          <p>Configurações</p>
        </div>
        <span className="chip">v{view.version}</span>
        {view.demo && <span className="chip chip-demo">Demonstração</span>}
      </header>

      {loadNotes[view.status] && (
        <p className="settings-note" role="status">
          {loadNotes[view.status]}
        </p>
      )}
      {error && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}

      <Section title="Geral">
        <div className="settings-row">
          <span>Iniciar com o Windows</span>
          <Switch
            checked={preferences.launchAtStartup}
            disabled={saving}
            label="Iniciar com o Windows"
            onChange={() =>
              onSave({
                ...preferences,
                launchAtStartup: !preferences.launchAtStartup,
              })
            }
          />
        </div>
        <p className="settings-hint">{startupNotes[view.startup]}</p>
        {view.startupMismatch && (
          <div className="settings-hint settings-warning" role="alert">
            <p>
              {preferences.launchAtStartup
                ? "A preferência está ativa, mas o Windows não tem a entrada deste aplicativo."
                : "A preferência está desativada, mas o Windows ainda tem a entrada deste aplicativo."}
            </p>
            <div className="settings-actions">
              <button
                type="button"
                className="ghost"
                disabled={saving}
                onClick={() => onSave(preferences)}
              >
                Corrigir
              </button>
            </div>
          </div>
        )}
        <div className="settings-row">
          <span>Mostrar a barra</span>
          <Switch
            checked={preferences.railVisible}
            disabled={saving}
            label="Mostrar a barra"
            onChange={() =>
              onSave({ ...preferences, railVisible: !preferences.railVisible })
            }
          />
        </div>
        {enabled === 0 && (
          <p className="settings-hint settings-warning">
            Nenhum provedor ativo: a barra fica oculta. Ative um provedor abaixo
            para exibi-la. A bandeja continua disponível.
          </p>
        )}
        <div className="settings-row">
          <span>Versão instalada</span>
          <span className="settings-value tabular-nums">{view.version}</span>
        </div>
      </Section>

      <Section
        title="Aparência e posicionamento"
        hint="Onde a barra semicircular fica na tela."
      >
        <div className="settings-row">
          <span>Lateral</span>
          <div className="segmented" role="radiogroup" aria-label="Lateral">
            {(["left", "right"] as const).map((side) => (
              <button
                key={side}
                type="button"
                role="radio"
                aria-checked={preferences.side === side}
                disabled={saving}
                onClick={() => onSave({ ...preferences, side })}
              >
                {side === "left" ? "Esquerda" : "Direita"}
              </button>
            ))}
          </div>
        </div>
        {view.sideOverride && (
          <p className="settings-hint">
            AURORA_SIDE define a lateral nesta sessão. A escolha salva vale no
            próximo início sem essa variável.
          </p>
        )}
        <div className="settings-row">
          <label htmlFor="monitor">Monitor</label>
          <select
            id="monitor"
            value={selected}
            disabled={saving}
            onChange={(event) =>
              onSave({
                ...preferences,
                monitor: chooseMonitor(view, event.target.value),
              })
            }
          >
            {options.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>
        {view.placement.fallback && (
          <p className="settings-hint settings-warning">
            O monitor escolhido está desconectado. A barra usa o monitor
            principal e volta ao escolhido quando ele for reconectado.
          </p>
        )}
        {view.placement.shown < enabled && (
          <p className="settings-hint settings-warning">
            A área útil deste monitor comporta {view.placement.shown} de{" "}
            {enabled} indicadores. Os primeiros da lista são exibidos.
          </p>
        )}
      </Section>

      <Section
        title="Provedores"
        hint="Provedores desativados não são consultados, não têm arquivos lidos e não geram alertas."
      >
        <div className="provider-list">
          {view.providers.map((provider) => (
            <ProviderCard
              key={provider.id}
              provider={provider}
              preferences={preferences}
              demo={view.demo}
              saving={saving}
              diagnostics={diagnostics[provider.id]}
              now={now}
              onSave={onSave}
              onDiagnose={() => onDiagnose(provider.id)}
            />
          ))}
        </div>
      </Section>

      <Section
        title="Notificações"
        hint="Notificações nativas do Windows. As preferências de notificação do sistema continuam valendo."
      >
        <div className="settings-row">
          <span>Ativar notificações</span>
          <Switch
            checked={notifications.enabled}
            disabled={saving}
            label="Ativar notificações"
            onChange={() =>
              saveNotifications({ enabled: !notifications.enabled })
            }
          />
        </div>

        <div className="settings-group" aria-label="Limites de consumo">
          <span className="settings-group-title">Alertar ao atingir</span>
          <div className="threshold-list">
            {view.thresholds.map((threshold) => (
              <button
                key={threshold}
                type="button"
                role="checkbox"
                aria-checked={notifications.thresholds.includes(threshold)}
                className="threshold"
                disabled={alertsOff}
                onClick={() =>
                  saveNotifications({
                    thresholds: toggleThreshold(
                      notifications.thresholds,
                      threshold,
                    ),
                  })
                }
              >
                {threshold}%
              </button>
            ))}
          </div>
        </div>

        {(
          [
            ["renewal", "Renovação confirmada de uma cota"],
            ["auth", "Sessão expirada, indisponível ou recusada"],
            ["reconnect", "Monitoramento restaurado após um problema"],
          ] as const
        ).map(([key, label]) => (
          <div className="settings-row" key={key}>
            <span>{label}</span>
            <Switch
              checked={notifications[key]}
              disabled={alertsOff}
              label={label}
              onChange={() => saveNotifications({ [key]: !notifications[key] })}
            />
          </div>
        ))}

        <div className="settings-group" aria-label="Alertas por provedor">
          <span className="settings-group-title">Alertas por provedor</span>
          {view.providers.map((provider) => {
            const choice = preferences.providers[provider.id];
            return (
              <div
                className="settings-row"
                key={provider.id}
                data-provider={provider.id}
              >
                <span className="provider-label">
                  <span className="provider-mark" aria-hidden="true" />
                  {provider.name}
                  {!choice.enabled && (
                    <span className="settings-muted"> · desativado</span>
                  )}
                </span>
                <Switch
                  checked={choice.alerts}
                  disabled={alertsOff}
                  label={`Alertas de ${provider.name}`}
                  onChange={() =>
                    onSave(
                      updateProvider(preferences, provider.id, {
                        alerts: !choice.alerts,
                      }),
                    )
                  }
                />
              </div>
            );
          })}
        </div>

        <div className="settings-row">
          <span>Horário silencioso</span>
          <Switch
            checked={quiet.enabled}
            disabled={alertsOff}
            label="Horário silencioso"
            onChange={() =>
              saveNotifications({
                quiet: { ...quiet, enabled: !quiet.enabled },
              })
            }
          />
        </div>
        {quiet.enabled && (
          <div className="quiet-hours">
            <ClockInput
              id="quiet-start"
              label="Das"
              value={quiet.start}
              disabled={alertsOff}
              onChange={(start) =>
                start !== quiet.end &&
                saveNotifications({ quiet: { ...quiet, start } })
              }
            />
            <ClockInput
              id="quiet-end"
              label="às"
              value={quiet.end}
              disabled={alertsOff}
              onChange={(end) =>
                end !== quiet.start &&
                saveNotifications({ quiet: { ...quiet, end } })
              }
            />
          </div>
        )}
        <p className="settings-hint">
          Durante o horário silencioso nada é exibido e nada fica acumulado para
          depois. O consumo continua sendo atualizado.
        </p>

        <div className="settings-actions">
          <button
            type="button"
            className="ghost"
            disabled={test === "sending"}
            onClick={onTestNotification}
          >
            Enviar notificação de teste
          </button>
          {test === "sent" && (
            <span className="settings-hint" role="status">
              Enviada. Se nada apareceu, verifique as notificações do Windows.
            </span>
          )}
          {test === "failed" && (
            <span className="settings-hint settings-warning" role="alert">
              O Windows não aceitou a notificação.
            </span>
          )}
        </div>
      </Section>

      <footer className="settings-footer">
        Preferências salvas localmente · formato v{preferences.version}
      </footer>
    </main>
  );
}
