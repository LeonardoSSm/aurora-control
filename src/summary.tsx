import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { UsageSummary } from "./components/UsageSummary";
import {
  onSummarySelection,
  showSummary,
  summaryReady,
} from "./desktop/commands";
import type { SummarySelection } from "./desktop/commands";
import { isNewerSelection } from "./desktop/ordering";
import "./styles.css";

const container = document.getElementById("root")!;
const root = createRoot(container);
let current: SummarySelection | null = null;

function render(selection: SummarySelection) {
  // Keyed by revision: fresh data for the same selection re-renders in place,
  // without replaying the reveal animation. Renewals are re-evaluated against
  // the current time on every render.
  flushSync(() =>
    root.render(
      <UsageSummary
        key={selection.revision}
        usage={selection.usage}
        side={selection.side}
        now={new Date()}
      />,
    ),
  );
}

void onSummarySelection((selection) => {
  // A late event (older selection, or older data of this selection) never
  // replaces what is on screen (audit I03).
  if (!isNewerSelection(current, selection)) return;
  current = selection;
  // Commit the card before asking Windows to reveal it, sized to its content.
  render(selection);
  const card = container.firstElementChild;
  const height = Math.ceil(card?.getBoundingClientRect().height ?? 0);
  void showSummary(selection.revision, height).catch(console.error);
})
  .then(() => summaryReady())
  .catch(console.error);

// Keep relative times ("há 2 min") and renewals current while the card is open.
setInterval(() => {
  if (current && document.visibilityState === "visible") render(current);
}, 30_000);
