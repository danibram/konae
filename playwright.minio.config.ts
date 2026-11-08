import { defineConfig } from "playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  testMatch: "**/minio-deployment.e2e.ts",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    // KONAE_E2E_GATEWAY_URL points the suite at a stack whose gateway publishes another port.
    baseURL: process.env.KONAE_E2E_GATEWAY_URL ?? "http://app.localhost:8080",
    channel: "chrome",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
});
