import { expect, test, type Page } from "playwright/test";

// Delivery I in the editor: Eject from the project list keeps the app serving,
// turns the project read-only, and an old tab's next edit is refused by the Kernel.
test.beforeEach(async ({ page }) => {
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await page.request.post("/_littlebox/owner/signup", { headers: { "x-forwarded-for": client }, data: { email: `owner-${crypto.randomUUID()}@example.test`, password: "Owner-test-password-2026", setupKey: "local-development-key" } });
  expect(response.ok()).toBe(true);
});

async function createProject(page: Page, name: string) {
  await page.goto("/projects");
  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill(name);
  await page.getByRole("radio", { name: /^Shared list/ }).check();
  const saved = page.waitForResponse((response) => new URL(response.url()).pathname === "/mcp" && response.ok() && (response.request().postData() ?? "").includes('"project_sources_save"'));
  await page.getByRole("button", { name: /^Create/ }).click();
  await expect(page).toHaveURL(/\/app\/[^/]+$/);
  await saved;
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0);
}

async function openFile(page: Page, path: string) {
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: "Files", exact: true }).click();
  await page.getByLabel("Open file").selectOption(path);
  return page.getByLabel(`${path} code editor`);
}

test("Eject from the project list keeps the app serving and refuses the old tab's next edit", async ({ page, context }) => {
  const name = `Eject tasks ${Date.now()}`;
  await createProject(page, name);
  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  await page.getByRole("button", { name: "Deploy public", exact: true }).click();
  const link = page.locator("a.published-url");
  await expect(link).toBeVisible({ timeout: 30_000 });
  const url = (await link.getAttribute("href"))!;
  await page.getByRole("button", { name: "Back to workshop" }).click();
  const app = await context.newPage();
  await app.goto(url);
  await app.getByPlaceholder(/Add one small thing/).fill("Written before Eject");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Written before Eject")).toBeVisible();

  // A second tab ejects while the first still has the editor open.
  const list = await context.newPage();
  await list.goto("/projects");
  const group = list.getByRole("region", { name: "Take it with you" });
  await group.getByRole("listitem").filter({ hasText: name }).getByRole("button", { name: "Eject…" }).click();
  const dialog = list.getByRole("dialog", { name: `Eject ${name}?` });
  await expect(dialog.getByText("The project goes from Managed to External.")).toBeVisible();
  await expect(dialog.getByText("Keeps URL, users, sessions, data, invitations and email service.")).toBeVisible();
  await expect(dialog.getByText("A conventional project that publishes artifacts through MCP.")).toBeVisible();
  await expect(dialog.getByText(/creates a new project, with no automatic merge of production data/)).toBeVisible();
  const download = list.waitForEvent("download");
  await dialog.getByRole("button", { name: "Download the code and eject" }).click();
  expect((await download).suggestedFilename()).toMatch(/-code\.tar$/);
  await expect(list.getByRole("dialog", { name: `${name} is External` })).toBeVisible();
  await list.getByRole("button", { name: "Done" }).click();
  const card = list.locator(".project-card").filter({ hasText: name });
  await expect(card.getByText("External", { exact: true })).toBeVisible();
  await expect(group.getByRole("listitem").filter({ hasText: name })).toHaveCount(0);

  // The old tab's edit reaches the Kernel and is refused with the Eject message.
  const editor = await openFile(page, "app.js");
  await editor.fill(`${await editor.inputValue()}\n// from an old tab`);
  const refusal = page.getByRole("banner").getByRole("alert");
  await expect(refusal).toContainText("Not saved", { timeout: 30_000 });
  await expect(refusal).toHaveAttribute("title", /This project was ejected/);

  // A fresh load opens the project read-only with the notice, and the app still has its row.
  await card.getByRole("button", { name: "View files" }).click();
  const frozen = await openFile(list, "app.js");
  await expect(frozen).toHaveAttribute("readonly", "");
  await expect(frozen).not.toContainText("from an old tab");
  await expect(list.getByRole("note", { name: "This project is External" }).first()).toContainText("bun run littlebox -- publish");
  await app.reload();
  await expect(app.getByText("Written before Eject")).toBeVisible();
});
