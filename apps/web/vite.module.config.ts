import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Builds a single module UI as an ES library. react/react-dom/react-router-dom are EXTERNAL — the
// browser resolves them via the shell's import map, so every module shares one React instance.
// Invoke per module: `vite build -c vite.module.config.ts` with MODULE_ID + MODULE_ENTRY env.
const id = process.env.MODULE_ID as string;
const entry = process.env.MODULE_ENTRY as string;
export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: `dist-modules/${id}`,
    emptyOutDir: true,
    lib: { entry, formats: ["es"], fileName: () => "index.js" },
    rollupOptions: { external: ["react", "react-dom", "react-dom/client", "react/jsx-runtime", "react-router-dom"] },
  },
});
