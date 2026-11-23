import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Rail } from "./components/Rail";
import { getUsage, onUsageUpdated, refreshUsage } from "./desktop/commands";
import { useRailMotion } from "./desktop/useRailMotion";
import type { RailState } from "./usage/presentation";
import "./styles.css";

function App() {
  const [state, setState] = useState<RailState>({ status: "loading" });
  const { frame, onPointer } = useRailMotion();

  useEffect(() => {
    let disposed = false;
    // Data updates only replace the snapshot; geometry and motion are untouched.
    const listener = onUsageUpdated((snapshot) => {
      if (!disposed) setState({ status: "ready", snapshot });
    });
    void listener
      .then(() => getUsage())
      .then(
        (snapshot) => {
          if (!disposed) setState({ status: "ready", snapshot });
        },
        (error: unknown) => {
          console.error(error);
          if (!disposed) setState({ status: "error" });
        },
      );
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
      shifts={frame.shifts}
      active={frame.active}
      onPointer={onPointer}
      onRefresh={(provider) => void refreshUsage(provider).catch(console.error)}
    />
  );
}

createRoot(document.getElementById("root")!).render(<App />);
