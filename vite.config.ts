import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// No other origin may frame the editor, so a published app cannot overlay it.
// The policy stops at framing. The preview is a srcdoc iframe, which inherits
// the editor's policy, and its inline scripts would stop under a script-src.
const frameHeaders = { "Content-Security-Policy": "frame-ancestors 'none'", "X-Frame-Options": "DENY" };

export default defineConfig({
  plugins: [react()],
  preview: { headers: frameHeaders },
  server: {
    port: 4173,
    headers: frameHeaders,
    proxy: {
      "/mcp": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/_littlebox": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/.well-known": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
      "/oauth": { target: process.env.LITTLEBOX_KERNEL_URL ?? "http://127.0.0.1:8787", changeOrigin: false },
    },
  },
});
