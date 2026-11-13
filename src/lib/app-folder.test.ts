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
