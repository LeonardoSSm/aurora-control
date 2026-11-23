import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { UsageSummary } from "./components/UsageSummary";
import {
  onSummarySelection,
  showSummary,
  summaryReady,
} from "./desktop/commands";
import type { SummarySelection } from "./desktop/commands";
import "./styles.css";

const container = document.getElementById("root")!;
const root = createRoot(container);
let current: SummarySelection | null = null;

function render(selection: SummarySelection) {
  // Keyed by revision: fresh data for the same selection re-renders in place,
  // without replaying the reveal animation.
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
  current = selection;
  // Commit the card before asking Windows to reveal it, sized to its content.
  render(selection);
  const card = container.firstElementChild;
  const height = Math.ceil(card?.getBoundingClientRect().height ?? 0);
  void showSummary(selection.revision, height).catch(console.error);
})
  .then(() => summaryReady())
  .catch(console.error);

// Keep relative times ("há 2 min") current while the card is open.
setInterval(() => {
  if (current && document.visibilityState === "visible") render(current);
}, 30_000);
