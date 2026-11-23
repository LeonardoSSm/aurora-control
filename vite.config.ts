import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    target: "edge111",
    rollupOptions: {
      input: {
        rail: fileURLToPath(new URL("./index.html", import.meta.url)),
        summary: fileURLToPath(new URL("./summary.html", import.meta.url)),
      },
    },
  },
});
