import { tmpdir } from "node:os";
import path from "node:path";
import { defineConfig } from "playwright/test";

const baseURL = process.env.KONAE_EDITOR_URL ?? "http://127.0.0.1:4176";
const editorPort = new URL(baseURL).port || "4176";
const kernelPort = process.env.KONAE_TEST_KERNEL_PORT ?? "8794";
const dataRoot = process.env.KONAE_E2E_DATA_DIR ?? path.join(tmpdir(), `konae-e2e-${process.pid}`);
process.env.KONAE_E2E_DATA_DIR = dataRoot;

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: ["**/local-platform.e2e.ts", "**/page-paths.e2e.ts"],
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    channel: "chrome",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    launchOptions: { args: ["--host-resolver-rules=MAP apps.example.org 127.0.0.1", `--unsafely-treat-insecure-origin-as-secure=http://apps.example.org:${kernelPort}`] },
  },
  webServer: {
    command: `VITE_KONAE_DURABLE_STAGING=true KONAE_TRUSTED_PROXIES=127.0.0.1 KONAE_MAIL_TRANSPORT=development KONAE_DOMAIN=apps.example.org KONAE_DATA_DIR='${dataRoot}' KONAE_EDITOR_PORT=${editorPort} KONAE_KERNEL_PORT=${kernelPort} bun run dev:local`,
    url: `${baseURL}/_konae/health`,
    reuseExistingServer: Boolean(process.env.KONAE_EDITOR_URL) && !process.env.CI,
    timeout: 120_000,
  },
});
