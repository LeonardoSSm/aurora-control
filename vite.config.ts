import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tests format local times. A fixed zone keeps them deterministic on any machine.
if (process.env.VITEST) process.env.TZ = "UTC";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    target: "edge111",
    rollupOptions: {
      input: {
        rail: fileURLToPath(new URL("./index.html", import.meta.url)),
        summary: fileURLToPath(new URL("./summary.html", import.meta.url)),
        settings: fileURLToPath(new URL("./settings.html", import.meta.url)),
      },
    },
  },
});
