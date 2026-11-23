import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Rail } from "./components/Rail";
import { getUsage } from "./desktop/commands";
import { useRailMotion } from "./desktop/useRailMotion";
import type { RailState } from "./usage/presentation";
import "./styles.css";

function App() {
  const [state, setState] = useState<RailState>({ status: "loading" });
  const { frame, onHover } = useRailMotion();

  useEffect(() => {
    void getUsage().then(
      (snapshot) => setState({ status: "ready", snapshot }),
      (error: unknown) => {
        console.error(error);
        setState({ status: "error" });
      },
    );
  }, []);

  if (!frame) return null;
  return (
    <Rail
      state={state}
      side={frame.placement.side}
      widths={frame.widths}
      onHover={onHover}
    />
  );
}

createRoot(document.getElementById("root")!).render(<App />);
