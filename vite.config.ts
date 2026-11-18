import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { devOwnerPlugin } from "./scripts/dev-owner";

// The policy stops at framing. The preview is a srcdoc iframe, which inherits
const frameHeaders = { "Content-Security-Policy": "frame-ancestors 'none'", "X-Frame-Options": "DENY" };

export default defineConfig({
  plugins: [react(), devOwnerPlugin(process.env.KONAE_KERNEL_URL ?? "http://127.0.0.1:8787", process.env.KONAE_DEV_OWNER_FILE)],
  optimizeDeps: { entries: ["index.html"] },
  preview: { headers: frameHeaders },
  server: {
    port: 4173,
    headers: frameHeaders,
    proxy: {
      "/mcp": { target: process.env.KONAE_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/_konae": { target: process.env.KONAE_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/.well-known": { target: process.env.KONAE_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/oauth": { target: process.env.KONAE_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
    },
  },
});
