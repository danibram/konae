import { tmpdir } from "node:os";
import path from "node:path";
import { defineConfig } from "playwright/test";

const baseURL = process.env.LITTLEBOX_EDITOR_URL ?? "http://127.0.0.1:4176";
const editorPort = new URL(baseURL).port || "4176";
const kernelPort = process.env.LITTLEBOX_TEST_KERNEL_PORT ?? "8794";
// Workers load this file again with their own pid, so the first load decides
// the path and the workers read it from the environment. The invitation test
// reads the development mailbox there.
const dataRoot = process.env.LITTLEBOX_E2E_DATA_DIR ?? path.join(tmpdir(), `littlebox-e2e-${process.pid}`);
process.env.LITTLEBOX_E2E_DATA_DIR = dataRoot;

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: ["**/local-platform.e2e.ts", "**/database-migration.e2e.ts"],
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    channel: "chrome",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: `LITTLEBOX_TRUSTED_PROXIES=127.0.0.1 LITTLEBOX_MAIL_TRANSPORT=development LITTLEBOX_DATA_DIR='${dataRoot}' LITTLEBOX_EDITOR_PORT=${editorPort} LITTLEBOX_KERNEL_PORT=${kernelPort} bun run dev:local`,
    url: `${baseURL}/_littlebox/health`,
    reuseExistingServer: Boolean(process.env.LITTLEBOX_EDITOR_URL) && !process.env.CI,
    timeout: 120_000,
  },
});
