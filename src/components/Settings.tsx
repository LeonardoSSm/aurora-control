import type { ReactNode } from "react";
import {
  chooseMonitor,
  monitorOptions,
  primaryValue,
  toggleProvider,
} from "../desktop/settings";
import type {
  Diagnostics,
  LoadStatus,
  Preferences,
  SettingsView,
  StartupStatus,
} from "../desktop/settings";
import { providerNames, providers } from "../usage/model";
import { formatMoment, issueText } from "../usage/presentation";

const loadNotes: Partial<Record<LoadStatus, string>> = {
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

export function Settings({
  view,
  diagnostics,
  saving,
  error,
  now,
  onSave,
  onRecheck,
}: {
  view: SettingsView;
  diagnostics: Diagnostics | null;
  saving: boolean;
  error: string | null;
  now: Date;
  onSave: (preferences: Preferences) => void;
  onRecheck: () => void;
}) {
  const preferences = view.preferences;
  const options = monitorOptions(view);
  const selected = preferences.monitor?.id ?? primaryValue;
  const visibleCount = providers.filter(
    (provider) => preferences.providers[provider],
  ).length;

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

      <Section title="Posição" hint="Onde a barra semicircular fica na tela.">
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
      </Section>

      <Section
        title="Provedores"
        hint="Provedores ocultos não são consultados nem têm arquivos lidos."
      >
        {providers.map((provider) => {
          const visible = preferences.providers[provider];
          const last = visible && visibleCount === 1;
          return (
            <div className="settings-row" key={provider}>
              <span className="provider-label" data-provider={provider}>
                <span className="provider-mark" aria-hidden="true" />
                {providerNames[provider]}
              </span>
              <Switch
                checked={visible}
                disabled={saving || last}
                label={`Mostrar ${providerNames[provider]}`}
                onChange={() => {
                  const next = toggleProvider(preferences, provider);
                  if (next) onSave(next);
                }}
              />
            </div>
          );
        })}
      </Section>

      <Section title="Sistema">
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
      </Section>

      <Section
        title="Diagnóstico do Claude Code"
        hint="Somente leitura. Nenhum token, conteúdo de arquivo ou identificador interno é exibido."
      >
        {diagnostics ? (
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
            {diagnostics.issue && (
              <li data-state="warning">
                <span className="diagnostic-dot" aria-hidden="true" />
                <span className="diagnostic-label">Última consulta</span>
                <span className="diagnostic-detail">
                  {issueText(diagnostics.issue, "claudeCode", now)}
                </span>
              </li>
            )}
          </ul>
        ) : (
          <p className="settings-hint">
            {view.demo
              ? "Modo de demonstração: nenhum arquivo do Claude Code é lido."
              : !preferences.providers.claudeCode
                ? "Claude Code oculto: nenhum arquivo dele é lido. Mostre-o para ver o diagnóstico."
                : "Diagnóstico indisponível."}
          </p>
        )}
        <div className="settings-actions">
          <button type="button" className="ghost" onClick={onRecheck}>
            Verificar novamente
          </button>
        </div>
      </Section>

      <footer className="settings-footer">
        Preferências salvas localmente · formato v{preferences.version}
      </footer>
    </main>
  );
}
