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
  build: {
    rollupOptions: {
      output: {
        // Pin React into stable named chunks so the import map in index.html can reference
        // /assets/react.js and /assets/react-router.js at known, unhashed paths.
        // Vite 8 / Rolldown requires manualChunks to be a function (not an object).
        manualChunks(id) {
          if (id.includes("node_modules/react-router")) return "react-router";
          if (
            id.includes("node_modules/react/") ||
            id.includes("node_modules/react-dom/") ||
            id.includes("node_modules/scheduler/")
          )
            return "react";
        },
        // Give the pinned chunks stable, hash-free names so the import map can reference them
        // without knowing the build hash. Only the react / react-router chunks lose the hash;
        // all others keep normal hashed names for cache-busting.
        chunkFileNames(chunkInfo) {
          if (chunkInfo.name === "react" || chunkInfo.name === "react-router") {
            return `assets/${chunkInfo.name}.js`;
          }
          return "assets/[name]-[hash].js";
        },
      },
    },
  },
});
