import { defineConfig } from "playwright/test";
import local from "./playwright.local.config";

export default defineConfig({
  ...local,
  use: { ...local.use, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true },
});
