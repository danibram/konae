import { expect, test } from "bun:test";
import { generatedFiles, type AppSettings } from "./app-folder";

const settings: AppSettings = {
  id: "notes-1", name: "Notes", slug: "notes", visibility: "private",
  signupPolicy: { mode: "invitation" }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1, routing: "pages" },
};

test("the app folder's AGENTS.md tells a person or agent to escape text written into the page", () => {
  const files = generatedFiles({ settings, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "" });
  const guide = files.find((file) => file.path === "AGENTS.md")?.contents as string;
  expect(guide).toContain("`textContent` or the template's `escapeHtml`, in attributes too");
});

test("the folder's .env.example names the image of the Kernel version its konae.json carries", () => {
  const entries = (kernel: string) => generatedFiles({ settings: { ...settings, kernel }, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "" });
  const env = (kernel: string) => entries(kernel).find((file) => file.path === ".env.example")?.contents as string;
  expect(env("0.6.0")).toContain("\nKONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:0.6.0\n");
  expect(env("0.7.1")).toContain("\nKONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:0.7.1\n");
  expect(env("0.6.0")).toContain("A release publishes this tag for linux/amd64 and linux/arm64. A local image can replace it.");
  const compose = entries("0.6.0").find((file) => file.path === "compose.yaml")?.contents as string;
  expect(compose).toContain("${KONAE_KERNEL_IMAGE:?");
});

test("the folder's AGENTS.md says when dist/ needs npm install and when it does not", () => {
  const files = generatedFiles({ settings, actionsSource: `{"version":1,"actions":{}}`, schemaSource: "", runtime: "", staticArtifact: "" });
  const guide = files.find((file) => file.path === "AGENTS.md")?.contents as string;
  const build = JSON.parse(files.find((file) => file.path === "package.json")?.contents as string).scripts.build;
  expect(build).toBe("node scripts/build.mjs");
  expect(guide).toContain("run `npm install` once, then `npm run build`");
  expect(guide).toContain("`dev` write it without `npm install`");
  expect(guide).toContain("until `node_modules` exists, then run `npm run build`");
});
