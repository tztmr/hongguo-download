import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    testTimeout: 10_000,
  },
  clearScreen: false,
  server: {
    port: 1424,
    strictPort: true,
    proxy: {
      "/api/studio": { target: "http://127.0.0.1:1426" },
    },
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1425,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
