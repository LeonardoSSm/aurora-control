import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Rail } from "./components/Rail";
import { getUsage, setHover } from "./desktop/commands";
import type { Provider } from "./usage/model";
import type { RailState } from "./usage/presentation";
import "./styles.css";

function App() {
  const [state, setState] = useState<RailState>({ status: "loading" });

  useEffect(() => {
    void getUsage().then(
      (snapshot) => setState({ status: "ready", snapshot }),
      (error: unknown) => {
        console.error(error);
        setState({ status: "error" });
      },
    );
  }, []);

  function handleHover(provider: Provider | null) {
    void setHover(provider).catch(console.error);
  }

  return <Rail state={state} onHover={handleHover} />;
}

createRoot(document.getElementById("root")!).render(<App />);
