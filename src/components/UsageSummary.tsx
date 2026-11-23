import type { Side } from "../desktop/railGeometry";
import { providerNames } from "../usage/model";
import type { Quota, Usage } from "../usage/model";
import {
  clampPercent,
  connection,
  connectionText,
  criticalPercent,
  formatReset,
  formatSpan,
  formatTokens,
  issueText,
  meter,
} from "../usage/presentation";

function Bar({ percent, tone }: { percent: number; tone: string }) {
  return (
    <div className="bar" data-tone={tone} aria-hidden="true">
      <div className="bar-fill" style={{ width: `${percent}%` }} />
    </div>
  );
}

function QuotaRow({ quota, now }: { quota: Quota; now: Date }) {
  const percent = clampPercent(quota.percentUsed);
  const reset =
    quota.kind === "credits"
      ? "Créditos adicionais"
      : formatReset(quota.resetsAt, now);
  return (
    <li className="quota">
      <div className="quota-head">
        <span className="quota-label">{quota.label}</span>
        <span className="quota-value tabular-nums">{percent}%</span>
      </div>
      <Bar
        percent={percent}
        tone={percent >= criticalPercent ? "critical" : "normal"}
      />
      <span className="quota-reset">{reset}</span>
    </li>
  );
}

export function UsageSummary({
  usage,
  side,
  now,
}: {
  usage: Usage;
  side: Side;
  now: Date;
}) {
  const reading = meter(usage);
  const state = connection(usage);
  const hasReading = usage.status !== "unavailable";

  return (
    <main
      className="summary select-none"
      data-provider={usage.provider}
      data-side={side}
    >
      <header className="summary-header">
        <span className="provider-mark" aria-hidden="true" />
        <h1 className="summary-title">{providerNames[usage.provider]}</h1>
        {usage.plan && <span className="chip">{usage.plan}</span>}
        {usage.source === "demo" && (
          <span className="chip chip-demo">Dados fictícios</span>
        )}
      </header>
      <p className="summary-account">
        {usage.account ?? "Conta não identificada"}
      </p>

      <section className="primary" aria-label="Consumo principal">
        <div className="primary-head">
          <strong
            className="primary-value tabular-nums"
            data-tone={reading.tone}
          >
            {reading.text}
          </strong>
          <span className="primary-label">
            {usage.window?.label ??
              (hasReading ? "Consumo" : "Consumo indisponível")}
          </span>
        </div>
        <Bar percent={reading.percent} tone={reading.tone} />
        {usage.window && (
          <p className="primary-reset">
            {formatReset(usage.window.resetsAt, now)}
          </p>
        )}
      </section>

      {usage.quotas.length > 0 && (
        <ul className="quotas" aria-label="Outras cotas">
          {usage.quotas.map((quota) => (
            <QuotaRow key={quota.id} quota={quota} now={now} />
          ))}
        </ul>
      )}

      {usage.activity && (
        <p className="activity">
          <span>
            Sessões locais · {formatSpan(usage.activity.windowMinutes * 60_000)}
          </span>
          <span className="tabular-nums">
            {formatTokens(usage.activity.tokens)} ·{" "}
            {usage.activity.sessions === 1
              ? "1 sessão"
              : `${usage.activity.sessions} sessões`}
          </span>
        </p>
      )}

      {usage.issue && (
        <p className="issue" role="alert">
          {issueText(usage.issue, usage.provider, now)}
        </p>
      )}

      <footer className="summary-footer" data-connection={state}>
        <span className="status-dot" aria-hidden="true" />
        <span>{connectionText(usage, now)}</span>
        {usage.refreshing && <span className="refreshing">Atualizando…</span>}
      </footer>
    </main>
  );
}
