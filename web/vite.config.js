import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  base: process.env.RVAH_WEB_BASE || "/",
  build: {
    target: "es2022",
    sourcemap: true,
    rollupOptions: {
      input: {
        app: fileURLToPath(new URL("./index.html", import.meta.url)),
        relay: fileURLToPath(new URL("./relay/index.html", import.meta.url)),
      },
    },
  },
  server: {
    headers: {
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  },
});
