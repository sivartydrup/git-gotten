import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // Vite options tailored for Tauri development.
  // 1. prevent Vite from obscuring Rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port; fail if it is not available
  server: {
    port: 1420,
    strictPort: true,
  },

  // The web root is `src/`; emit the production bundle to the repo-root `dist/`
  // so Tauri's `frontendDist` (`../../dist`) finds it. `emptyOutDir` must be set
  // explicitly because the outDir lives outside the Vite root.
  build: {
    outDir: "../dist",
    emptyOutDir: true,
  },

  // Unit tests run in a DOM environment (there is no real webview in Vitest).
  test: {
    environment: "jsdom",
  },
}));
