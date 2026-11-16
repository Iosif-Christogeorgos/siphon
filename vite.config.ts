import { realpathSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The project folder spelled the way the file system spells it. Windows
// treats c:\ and C:\ as one folder, but Vite compares paths as text: started
// from a lowercase path (as some editors do), it bundled React twice in dev
// and the window stayed blank with "Invalid hook call".
const root = realpathSync.native(fileURLToPath(new URL(".", import.meta.url)));

// Tauri drives the dev server: it expects a fixed port and its own Rust
// output must not trigger frontend reloads.
export default defineConfig({
  root,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": join(root, "src") },
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
