import path from "node:path";
import { tmpdir } from "node:os";
import { defineConfig } from "playwright/test";

const root = path.resolve(import.meta.dirname, "../..");
const baseURL = process.env.KONAE_EDITOR_URL ?? "http://127.0.0.1:4176";
const port = new URL(baseURL).port || "4176";
const dataRoot = process.env.KONAE_E2E_DATA_DIR ?? path.join(tmpdir(), `konae-e2e-durable-mvp-${process.pid}`);
process.env.KONAE_E2E_DATA_DIR = dataRoot;

export default defineConfig({
  testDir: ".",
  testMatch: ["editor-mobile-profile.e2e.ts"],
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],
  outputDir: path.join(dataRoot, "playwright-artifacts"),
  use: {
    baseURL,
    channel: "chrome",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    launchOptions: { args: ["--host-resolver-rules=MAP apps.example.org 127.0.0.1"] },
  },
  webServer: {
    command: `env -i KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT=${port} KONAE_DATA_DIR='${dataRoot}' KONAE_PUBLIC_ORIGIN='${baseURL}' KONAE_HOST_SUFFIX=localhost KONAE_SETUP_KEY=local-development-key KONAE_TRUSTED_PROXIES=127.0.0.1 KONAE_MAIL_TRANSPORT=development KONAE_EDITOR_DIR='${path.join(root, "dist")}' '${path.join(root, "kernel-rs/target/debug/konae")}'`,
    url: `${baseURL}/_konae/health`,
    reuseExistingServer: false,
    timeout: 60_000,
    cwd: root,
  },
});
