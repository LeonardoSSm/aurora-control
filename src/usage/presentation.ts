import { providerNames } from "./model";
import type { Issue, Provider, Snapshot, Usage } from "./model";

export type RailState =
  { status: "loading" | "error" } | { status: "ready"; snapshot: Snapshot };

export function railStatus(state: RailState) {
  switch (state.status) {
    case "loading":
      return { label: "Carregando utilização" };
    case "error":
      return { label: "Falha ao carregar a utilização" };
    case "ready":
      return state.snapshot.some((usage) => usage.source === "demo")
        ? { label: "Dados fictícios" }
        : { label: "Utilização" };
  }
}

/** At or above this share, the ring switches to the critical tone. */
export const criticalPercent = 90;

export type Tone = "normal" | "critical" | "stale" | "none";

export function clampPercent(value: number) {
  return Math.round(Math.min(100, Math.max(0, value)));
}

export function meter(usage?: Usage) {
  if (!usage || usage.status === "unavailable") {
    return {
      percent: 0,
      text: "—",
      label: "Utilização indisponível",
      tone: "none" as Tone,
    };
  }
  const percent = clampPercent(usage.percentUsed);
  const text = `${percent}%`;
  const stale = usage.status === "stale";
  const windowLabel = usage.window
    ? `, ${usage.window.label.toLowerCase()}`
    : "";
  const tone: Tone = stale
    ? "stale"
    : percent >= criticalPercent
      ? "critical"
      : "normal";
  return {
    percent,
    text,
    label: `${text} utilizado${windowLabel}${stale ? ", dado desatualizado" : ""}`,
    tone,
  };
}

/** Compact window tag drawn under the percentage, such as `5 h` or `7 d`. */
export function shortDuration(minutes: number | null | undefined) {
  if (minutes == null) return "";
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 1440) return `${Math.round(minutes / 60)} h`;
  return `${Math.round(minutes / 1440)} d`;
}

const clock = new Intl.DateTimeFormat("pt-BR", {
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

const dayAndClock = new Intl.DateTimeFormat("pt-BR", {
  weekday: "short",
  day: "2-digit",
  month: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

function parse(value: string | null | undefined) {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? null : date;
}

function sameDay(a: Date, b: Date) {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/** Local time, with the weekday and date when it is not today. */
export function formatMoment(value: string | null | undefined, now: Date) {
  const date = parse(value);
  if (!date) return "—";
  return (sameDay(date, now) ? clock : dayAndClock)
    .format(date)
    .replace(".,", ",");
}

/** A span such as `2 h 14 min` or `3 d 4 h`. */
export function formatSpan(milliseconds: number) {
  if (milliseconds < 60_000) return "menos de 1 min";
  const minutes = Math.round(milliseconds / 60_000);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return rest ? `${hours} h ${rest} min` : `${hours} h`;
  }
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return rest ? `${days} d ${rest} h` : `${days} d`;
}

export function formatReset(value: string | null | undefined, now: Date) {
  const date = parse(value);
  if (!date) return "Renovação desconhecida";
  const remaining = date.valueOf() - now.valueOf();
  if (remaining <= 0) return "Renovada";
  return `Renova em ${formatSpan(remaining)} · ${formatMoment(value, now)}`;
}

export function formatAgo(value: string | null | undefined, now: Date) {
  const date = parse(value);
  if (!date) return null;
  const elapsed = now.valueOf() - date.valueOf();
  return elapsed < 60_000 ? "agora" : `há ${formatSpan(elapsed)}`;
}

export function formatDuration(minutes: number | null | undefined) {
  if (minutes == null) return "—";
  return formatSpan(minutes * 60_000);
}

const compact = new Intl.NumberFormat("pt-BR", {
  notation: "compact",
  maximumFractionDigits: 1,
});

export function formatTokens(tokens: number) {
  return `${compact.format(tokens)} tokens`;
}

/** What went wrong, in terms of what the user can do about it. */
export function issueText(issue: Issue, provider: Provider, now: Date) {
  const name = providerNames[provider];
  switch (issue.code) {
    case "authMissing":
      return provider === "codex"
        ? "Nenhuma sessão do Codex encontrada. Entre com “codex login”."
        : "Nenhuma sessão do Claude Code encontrada. Execute “claude” no terminal.";
    case "authExpired":
      return provider === "codex"
        ? "A sessão do Codex expirou. Abra o Codex para renová-la."
        : "A sessão do Claude Code expirou. Execute “claude” no terminal para renová-la.";
    case "authRejected":
      return `O ${name} recusou a sessão local. Entre novamente.`;
    case "unsupported":
      return provider === "codex"
        ? "Login por chave de API: o Codex não expõe cotas de assinatura."
        : "Login por chave de API ou token restrito: cotas e custos não são expostos localmente.";
    case "rateLimited":
      return issue.retryAt
        ? `Limite de consultas atingido. Nova tentativa ${formatMoment(issue.retryAt, now)}.`
        : "Limite de consultas atingido. Nova tentativa em breve.";
    case "network":
      return "Sem conexão com o provedor.";
    case "timeout":
      return "O provedor não respondeu a tempo.";
    case "malformed":
      return "Resposta inesperada do provedor. O formato pode ter mudado.";
    case "http":
      return `O provedor respondeu com erro ${issue.status}.`;
  }
}

export type Connection = "live" | "stale" | "offline" | "demo";

export function connection(usage: Usage): Connection {
  if (usage.source === "demo") return "demo";
  if (usage.status === "available") return "live";
  if (usage.status === "stale") return "stale";
  return "offline";
}

export function connectionText(usage: Usage, now: Date) {
  const ago = formatAgo(usage.updatedAt, now);
  switch (connection(usage)) {
    case "demo":
      return ago ? `Demonstração · atualizado ${ago}` : "Demonstração";
    case "live":
      return ago ? `Conectado · atualizado ${ago}` : "Conectado";
    case "stale": {
      const from =
        usage.origin === "sessionLog" ? "registro local" : "última leitura";
      return ago ? `Desatualizado · ${from} ${ago}` : "Desatualizado";
    }
    case "offline":
      return "Indisponível";
  }
}
