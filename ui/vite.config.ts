import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and a relative base so the built assets load
// from the app bundle. See docs/03_ARCHITECTURE.md.
export default defineConfig({
  plugins: [react()],
  base: "./",
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", target: "es2021", sourcemap: true },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: [],
  },
});
