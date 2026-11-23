import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Rail } from "./components/Rail";
import { getUsage, onUsageUpdated, refreshUsage } from "./desktop/commands";
import { isNewerUpdate } from "./desktop/ordering";
import { useRailMotion } from "./desktop/useRailMotion";
import type { UsageUpdate } from "./usage/model";
import type { RailState } from "./usage/presentation";
import "./styles.css";

function App() {
  const [state, setState] = useState<RailState>({ status: "loading" });
  const { frame, onPointer } = useRailMotion();

  useEffect(() => {
    let disposed = false;
    let shown: number | null = null;
    // Updates and the initial read are numbered by Rust: an older one that
    // arrives late never replaces a newer snapshot.
    const accept = (update: UsageUpdate) => {
      if (disposed || !isNewerUpdate(shown, update.sequence)) return;
      shown = update.sequence;
      setState({ status: "ready", snapshot: update.snapshot });
    };
    const listener = onUsageUpdated(accept);
    void listener
      .then(() => getUsage())
      .then(accept, (error: unknown) => {
        console.error(error);
        if (!disposed && shown === null) setState({ status: "error" });
      });
    return () => {
      disposed = true;
      void listener.then((unlisten) => unlisten()).catch(console.error);
    };
  }, []);

  if (!frame) return null;
  return (
    <Rail
      state={state}
      side={frame.placement.side}
      providers={frame.placement.providers}
      shifts={frame.shifts}
      active={frame.active}
      onPointer={onPointer}
      onRefresh={(provider) => void refreshUsage(provider).catch(console.error)}
    />
  );
}

createRoot(document.getElementById("root")!).render(<App />);
