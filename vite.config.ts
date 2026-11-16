import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri drives the dev server: it expects a fixed port and its own Rust
// output must not trigger frontend reloads.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    // The bundle loads from disk inside the app, so one chunk is the right
    // trade-off; the default 500 kB warning is tuned for the web.
    chunkSizeWarningLimit: 1000,
  },
});
