import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 4173,
    proxy: {
      "/mcp": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/_littlebox": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/.well-known": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/oauth": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
    },
  },
});
