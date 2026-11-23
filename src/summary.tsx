import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { UsageSummary } from "./components/UsageSummary";
import {
  onSummarySelection,
  showSummary,
  summaryReady,
} from "./desktop/commands";
import "./styles.css";

const root = createRoot(document.getElementById("root")!);

void onSummarySelection(({ usage, revision }) => {
  // Commit the card before asking Windows to reveal it, including on the first hover.
  flushSync(() => root.render(<UsageSummary usage={usage} />));
  void showSummary(revision).catch(console.error);
})
  .then(() => summaryReady())
  .catch(console.error);
