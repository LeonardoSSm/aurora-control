import type { Snapshot, Usage } from "./model";

export type RailState =
  { status: "loading" | "error" } | { status: "ready"; snapshot: Snapshot };

export function railStatus(state: RailState) {
  switch (state.status) {
    case "loading":
      return { text: "…", label: "Carregando utilização" };
    case "error":
      return { text: "Falha", label: "Falha ao carregar a utilização" };
    case "ready":
      return state.snapshot.some((usage) => usage.source === "demo")
        ? { text: "Demo", label: "Dados fictícios" }
        : { text: "", label: "Utilização" };
  }
}

export function meter(usage?: Usage) {
  if (!usage || usage.status === "unavailable") {
    return { percent: 0, text: "—", label: "Utilização indisponível" };
  }

  const percent = Math.round(Math.min(100, Math.max(0, usage.percentUsed)));
  const text = `${percent}%`;
  const status = usage.status === "stale" ? ", dado desatualizado" : "";
  return { percent, text, label: `${text} utilizado${status}` };
}

const dateTime = new Intl.DateTimeFormat("pt-BR", {
  day: "2-digit",
  month: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

export function formatTime(value: string | null | undefined) {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? "—" : dateTime.format(date);
}

export function formatDuration(minutes: number | undefined) {
  if (minutes === undefined) return "—";
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  if (!hours) return `${remainder} min`;
  return remainder ? `${hours} h ${remainder} min` : `${hours} h`;
}
