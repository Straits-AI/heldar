import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Dev server proxies the API surface to the Rust core on :8000 so the SPA can
// talk to it with same-origin relative paths (no CORS, no hard-coded host).
const CORE = "http://localhost:8000";

export default defineConfig({
  // Served at `/` on the appliance (same-origin with the kernel); built with VITE_BASE_PATH=/app/ when
  // hosted as the remote dashboard behind the rendezvous Worker (ADR 0003 P3, Stage B).
  base: process.env.VITE_BASE_PATH || "/",
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target: CORE, changeOrigin: true },
      "/media": { target: CORE, changeOrigin: true },
      "/healthz": { target: CORE, changeOrigin: true },
    },
  },
});
