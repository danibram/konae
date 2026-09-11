import { expect, test, type Page } from "playwright/test";

async function createProject(page: Page, name: string, template = "tasks") {
  await page.goto("/projects");
  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill(name);
  await page.getByLabel("Project template").selectOption(template);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
}

async function publish(page: Page) {
  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  await page.getByRole("button", { name: "Deploy public", exact: true }).click();
  const link = page.locator("a.published-url");
  await expect(link).toBeVisible({ timeout: 30_000 });
  const url = await link.getAttribute("href");
  await page.getByRole("button", { name: "Back to workshop" }).click();
  return url!;
}

test("browser builds, republishes, backs up and exports a persistent app", async ({ page, context }, testInfo) => {
  await createProject(page, `Browser tasks ${Date.now()}`);
  const explorer = page.getByRole("navigation", { name: "Project files" });
  await explorer.getByRole("button", { name: "New file" }).click();
  await page.getByLabel("File path").fill("src/label.js");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.getByLabel("src/label.js code editor").fill("export const label = 'Compiled in the browser';");
  await explorer.getByRole("button", { name: "app.js", exact: true }).click();
  const editor = page.getByLabel("app.js code editor");
  const source = await editor.inputValue();
  await editor.fill(`import { label } from './src/label.js';\n${source.replace("Nothing here. Lovely.", "' + label + '")}`);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  await page.reload();
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  const url = await publish(page);
  const app = await context.newPage();
  await app.goto(url);
  await expect(app.getByText("Compiled in the browser")).toBeVisible();
  await app.getByPlaceholder(/Add one small thing/).fill("Survives a new release");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Survives a new release")).toBeVisible();
  expect(await publish(page)).toBe(url);
  await app.reload();
  await expect(app.getByText("Survives a new release")).toBeVisible();

  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Data/i }).click();
  await page.getByLabel("Database target").selectOption("deployed");
  await page.getByRole("navigation", { name: "Database views" }).getByRole("button", { name: "Backups" }).click();
  await page.getByRole("button", { name: "Create backup" }).click();
  await expect(page.getByRole("status")).toContainText("Backup created");
  await app.getByPlaceholder(/Add one small thing/).fill("Added after backup");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Added after backup")).toBeVisible();
  await page.getByRole("listitem").filter({ hasText: "Manual backup" }).getByRole("button", { name: "Restore", exact: true }).click();
  await page.getByRole("button", { name: "Confirm restore" }).click();
  await expect(page.getByRole("status")).toContainText("Backup restored");
  await app.reload();
  await expect(app.getByText("Survives a new release")).toBeVisible();
  await expect(app.getByText("Added after backup")).toHaveCount(0);

  const downloadEvent = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export project", exact: true }).click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toMatch(/-selfhost\.tar$/);
  await download.saveAs(testInfo.outputPath("selfhost.tar"));
  await app.screenshot({ path: testInfo.outputPath("published-app.png"), fullPage: true });
});

test("public notes authenticate users and isolate private rows", async ({ page, context }, testInfo) => {
  await createProject(page, `Personal notes ${Date.now()}`, "notes");
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await preview.getByRole("button", { name: "Sign in", exact: true }).click();
  await preview.getByPlaceholder("Keep a thought…").fill("Alice preview note");
  await preview.getByRole("button", { name: "Save", exact: true }).click();
  await expect(preview.getByText("Alice preview note")).toBeVisible();
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Auth/i }).click();
  await page.getByLabel("Preview user").selectOption("bob");
  await expect(preview.getByText("No notes yet. Start with one thought.")).toBeVisible();
  await expect(preview.getByText("Alice preview note")).toHaveCount(0);
  await page.getByLabel("Preview user").selectOption("alice");
  await expect(preview.getByText("Alice preview note")).toBeVisible();
  const url = await publish(page);
  const app = await context.newPage();
  await app.goto(url);
  await expect(app.getByRole("button", { name: "Sign in", exact: true })).toBeVisible();
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await app.getByLabel("Email").fill("alice@example.test");
  await app.getByLabel("Password").fill("Local-test-password-943");
  await app.getByRole("button", { name: "Create account", exact: true }).click();
  await expect(app.getByPlaceholder("Keep a thought…")).toBeVisible();
  await app.getByPlaceholder("Keep a thought…").fill("Alice published note");
  await app.getByRole("button", { name: "Save", exact: true }).click();
  await expect(app.getByText("Alice published note")).toBeVisible();
  await app.getByRole("button", { name: "Sign out", exact: true }).click();
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await app.getByLabel("Email").fill("bob@example.test");
  await app.getByLabel("Password").fill("Local-test-password-943");
  await app.getByRole("button", { name: "Create account", exact: true }).click();
  await expect(app.getByText("No notes yet. Start with one thought.")).toBeVisible();
  await expect(app.getByText("Alice published note")).toHaveCount(0);
  await app.getByRole("button", { name: "Sign out", exact: true }).click();
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await app.getByLabel("Email").fill("alice@example.test");
  await app.getByLabel("Password").fill("Local-test-password-943");
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(app.getByText("Alice published note")).toBeVisible();
  await app.reload();
  await expect(app.getByText("Alice published note")).toBeVisible();
  await app.screenshot({ path: testInfo.outputPath("authenticated-notes.png"), fullPage: true });
  const downloadEvent = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export project", exact: true }).click();
  const download = await downloadEvent;
  await download.saveAs(testInfo.outputPath("notes-selfhost.tar"));
});

test("local and deployed migrations track their own model state", async ({ page, context }) => {
  await createProject(page, `Separate database models ${Date.now()}`);
  const url = await publish(page);
  const app = await context.newPage();
  await app.goto(url);
  await app.getByPlaceholder(/Add one small thing/).fill("Keep published data");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Keep published data")).toBeVisible();
  await page.getByRole("navigation", { name: "Project files" }).getByRole("button", { name: "schema.ts", exact: true }).click();
  const model = page.getByLabel("schema.ts code editor");
  await model.fill((await model.inputValue()).replace('  title: text("title").notNull(),', '  title: text("title").notNull(),\n  priority: text("priority").notNull().default("normal"),'));
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name: /Data/i }).click();
  await page.getByLabel("Database target").selectOption("local");
  const views = page.getByRole("navigation", { name: "Database views" });
  await views.getByRole("button", { name: "Schema", exact: true }).click();
  await page.getByRole("button", { name: "Apply migration", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Migration applied");
  await page.getByLabel("Database target").selectOption("deployed");
  await expect(page.getByRole("region", { name: "Migration plan" })).toContainText("priority");
  await page.getByRole("button", { name: "Apply migration", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("Migration applied");
  await views.getByRole("button", { name: "Data", exact: true }).click();
  await expect(page.locator(".editable-grid")).toContainText("Keep published data");
  await expect(page.locator(".editable-grid")).toContainText("normal");
  expect(await publish(page)).toBe(url);
  await app.reload();
  await expect(app.getByText("Keep published data")).toBeVisible();
});
