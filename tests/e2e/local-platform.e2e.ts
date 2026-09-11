import { expect, test, type Page } from "playwright/test";

test.beforeEach(async ({ page }) => {
  // The Kernel limits owner signups per client address. The test server trusts the local
  // proxy, so give each test its own forwarded address and repeated runs stay under the limit.
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await page.request.post("/_littlebox/owner/signup", { headers: { "x-forwarded-for": client }, data: { email: `owner-${crypto.randomUUID()}@example.test`, password: "Owner-test-password-2026", setupKey: "local-development-key" } });
  expect(response.ok()).toBe(true);
});

test("platform pages require an owner session and return to the requested page after sign-in", async ({ page }) => {
  const email = `owner-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const client = `203.0.113.${1 + Math.floor(Math.random() * 254)}`;
  const signup = await page.request.post("/_littlebox/owner/signup", { headers: { "x-forwarded-for": client }, data: { email, password, setupKey: "local-development-key" } });
  expect(signup.ok()).toBe(true);
  await page.request.post("/_littlebox/owner/logout");

  for (const path of ["/", "/app/app_tiny_tasks", "/design-system"]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/platform-access\?returnTo=/);
    await expect(page.getByRole("heading", { name: "Owner sign-in" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Workspace sections" })).toHaveCount(0);
  }

  await page.goto("/app/app_tiny_tasks");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/app\/app_tiny_tasks$/);
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
});

async function signInApp(page: Page) {
  await expect(page.getByRole("heading", { name: "Sign in to this app" })).toBeVisible();
  await page.getByLabel("Email").fill(`app-${crypto.randomUUID()}@example.test`);
  await page.getByLabel("Password").fill("App-test-password-2026");
  await page.getByRole("button", { name: "Create account", exact: true }).click();
}

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
  await editor.fill(`import { label } from './src/label.js';\n${source.replace("Nothing here yet. Anyone with this link sees the same list.", "' + label + '")}`);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  await page.reload();
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  const url = await publish(page);
  const app = await context.newPage();
  await app.goto(url);
  // The tasks starter declares anonymous actions, so a signed-out visitor of a public app can use them.
  await expect(app.getByText("Compiled in the browser")).toBeVisible();
  await app.getByPlaceholder(/Add one small thing/).fill("Survives a new release");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Survives a new release")).toBeVisible();
  // Uploads still need a session.
  await app.goto(new URL("/_littlebox/login", url!).href);
  await signInApp(app);
  await expect(app.getByText("Survives a new release")).toBeVisible();
  const attachmentType = await app.evaluate(async () => {
    const sdk = (window as unknown as { miniapp: { files: { upload(file: File): Promise<{ url: string }> } } }).miniapp;
    const attachment = await sdk.files.upload(new File(['<script>document.body.dataset.executed="yes"</script>'], "untrusted.html", { type: "text/html" }));
    return (await fetch(attachment.url)).headers.get("content-type");
  });
  expect(attachmentType).toBe("application/octet-stream");
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
  // The template's web font swaps in late and reflows the heading above the
  // button, so a click aimed before the swap can land on empty space. Signing
  // in twice is harmless, so retry until the notes composer shows.
  const composer = preview.getByPlaceholder("Keep a thought…");
  await expect(async () => {
    if (!(await composer.isVisible())) await preview.getByRole("button", { name: "Sign in", exact: true }).click({ timeout: 2_000 });
    await expect(composer).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 20_000 });
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
