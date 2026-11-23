import type { Side } from "../desktop/railGeometry";
import { providerNames } from "../usage/model";
import type { Usage } from "../usage/model";
import { formatDuration, formatTime, meter } from "../usage/presentation";

export function UsageSummary({ usage, side }: { usage: Usage; side: Side }) {
  const reading = meter(usage);
  const stateText = {
    available: "da cota utilizada",
    stale: "Dado desatualizado",
    unavailable: "Indisponível",
  }[usage.status];

  return (
    <main
      className="summary h-screen select-none"
      data-provider={usage.provider}
      data-side={side}
    >
      <header className="summary-header flex items-start justify-between gap-2">
        <div className="min-w-0">
          <h1 className="summary-title">{providerNames[usage.provider]}</h1>
          <p className="summary-account truncate">
            {usage.account ?? "Conta indisponível"}
          </p>
        </div>
        {usage.source === "demo" && (
          <span className="summary-badge shrink-0">Dados fictícios</span>
        )}
      </header>
      <div className="summary-reading flex items-baseline gap-2">
        <strong className="summary-value tabular-nums">{reading.text}</strong>
        <span>{stateText}</span>
      </div>
      <dl className="summary-details grid grid-cols-[1fr_auto] gap-x-2 tabular-nums">
        <dt>Janela de consumo</dt>
        <dd>{formatDuration(usage.window?.durationMinutes)}</dd>
        <dt>Renovação</dt>
        <dd>{formatTime(usage.window?.resetsAt)}</dd>
        <dt>Atualização</dt>
        <dd>{formatTime(usage.updatedAt)}</dd>
      </dl>
    </main>
  );
}
