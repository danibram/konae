import { readFile } from "node:fs/promises";
import path from "node:path";
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
  // A fresh owner has no projects, so a link to the browser starter would fall back to the
  // project list. Create a project first so the requested page is a real workspace.
  const projectPath = await createProject(page, `Return here ${Date.now()}`);
  await page.request.post("/_littlebox/owner/logout");

  for (const path of ["/", projectPath, "/design-system"]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/platform-access\?returnTo=/);
    await expect(page.getByRole("heading", { name: "Owner sign-in" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Workspace sections" })).toHaveCount(0);
  }

  await page.goto(projectPath);
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
});

async function signInApp(page: Page) {
  await expect(page.getByRole("heading", { name: "Sign in to this app" })).toBeVisible();
  await page.getByLabel("Email").fill(`app-${crypto.randomUUID()}@example.test`);
  await page.getByLabel("Password").fill("App-test-password-2026");
  await page.getByRole("button", { name: "Create account", exact: true }).click();
}

const templates = { tasks: "Shared list", notes: "Private notes" } as const;

async function createProject(page: Page, name: string, template: keyof typeof templates = "tasks") {
  await page.goto("/projects");
  await page.getByRole("button", { name: "New project" }).click();
  await page.getByLabel("Project name").fill(name);
  await page.getByRole("radio", { name: new RegExp(`^${templates[template]}`) }).check();
  // The new project reaches the Kernel after a short debounce. Wait for that save so a
  // test that logs out or reloads right after does not lose it.
  const saved = page.waitForResponse((response) => new URL(response.url()).pathname === "/mcp" && response.ok() && (response.request().postData() ?? "").includes('"project_sources_save"'));
  await page.getByRole("button", { name: /^Create/ }).click();
  await expect(page).toHaveURL(/\/app\/[^/]+$/);
  await expect(page.getByRole("banner").locator("summary").getByText(name, { exact: true })).toBeVisible();
  await saved;
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0);
  return new URL(page.url()).pathname;
}

// The editor sends project sources to the Kernel 500 ms after the last edit (App.tsx) and
// shows "Saving" until the Kernel has them. A reload before that loads the older sources.
async function waitForKernelSave(page: Page) {
  await page.waitForTimeout(1_000);
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByRole("banner").getByText("Not saved")).toHaveCount(0);
}

async function showSection(page: Page, name: "Preview" | "Auth" | "Files" | "Data") {
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("button", { name, exact: true }).click();
}

// Files has a File list button that opens New file and New folder beside the editor.
async function openExplorer(page: Page) {
  await showSection(page, "Files");
  await page.getByRole("button", { name: "File list", exact: true }).click();
  const explorer = page.getByRole("navigation", { name: "Project files" });
  await expect(explorer).toBeVisible();
  return explorer;
}

async function openFile(page: Page, path: string) {
  await showSection(page, "Files");
  await page.getByLabel("Open file").selectOption(path);
  return page.getByLabel(`${path} code editor`);
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
  const explorer = await openExplorer(page);
  // The button closes the File list and the fold shortcut still brings it back.
  await page.getByRole("button", { name: "File list", exact: true }).click();
  await expect(explorer).toBeHidden();
  await page.keyboard.press("Control+b");
  await expect(explorer).toBeVisible();
  await explorer.getByRole("button", { name: "New file" }).click();
  await page.getByLabel("File path").fill("src/label.js");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  // The new file opens in the editor and the File list stays beside it.
  await page.getByLabel("src/label.js code editor").fill("export const label = 'Compiled in the browser';");
  await expect(explorer).toBeVisible();
  // The Action Contract is generated from the Capsule and cannot be edited.
  await explorer.getByRole("button", { name: "action-contract.md" }).click();
  const contract = page.getByLabel("action-contract.md, generated, read only");
  await expect(contract).toHaveValue(/^# Action Contract for Browser tasks/);
  await expect(contract).toHaveAttribute("readonly", "");
  await expect(page.getByText("Generated from actions.json and schema.sql, so it cannot be edited here.")).toBeVisible();
  await expect(explorer.getByRole("button", { name: "Delete selected path" })).toBeDisabled();
  await explorer.getByRole("button", { name: "app.js" }).click();
  const editor = page.getByLabel("app.js code editor");
  await expect(explorer).toBeVisible();
  const source = await editor.inputValue();
  await editor.fill(`import { label } from './src/label.js';\n${source.replace("Nothing here yet. Anyone with this link sees the same list.", "' + label + '")}`);
  // Run shows the rebuilt preview without a trip to the Preview section.
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  await waitForKernelSave(page);
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
  // The app's CSP keeps connect-src to its own origin, so the SDK's blob URL cannot be fetched.
  // The Kernel's file path on the same origin must hand the HTML back as a download, never a page.
  const attachment = await app.evaluate(async () => {
    const sdk = (window as unknown as { miniapp: { files: { upload(file: File): Promise<{ id: string }> } } }).miniapp;
    const stored = await sdk.files.upload(new File(['<script>document.body.dataset.executed="yes"</script>'], "untrusted.html", { type: "text/html" }));
    const response = await fetch(`/_littlebox/files/${stored.id}`);
    return { status: response.status, disposition: response.headers.get("content-disposition"), sniffing: response.headers.get("x-content-type-options") };
  });
  expect(attachment).toEqual({ status: 200, disposition: "attachment", sniffing: "nosniff" });
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
  await page.getByRole("button", { name: "Export", exact: true }).click();
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
  // One section shows at a time, so pick the preview user in Auth and look in Preview.
  await showSection(page, "Auth");
  // The heading agrees with the template's rule: only a signed-in person lists notes.
  await expect(page.getByRole("heading", { name: "A signed-in person can list the rows. The list stops at 100 rows." })).toBeVisible();
  await page.getByLabel("Preview user").selectOption("bob");
  await showSection(page, "Preview");
  await expect(preview.getByText("No notes yet. Start with one thought.")).toBeVisible();
  await expect(preview.getByText("Alice preview note")).toHaveCount(0);
  await showSection(page, "Auth");
  await page.getByLabel("Preview user").selectOption("alice");
  await showSection(page, "Preview");
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
  await page.getByRole("button", { name: "Export", exact: true }).click();
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
  const model = await openFile(page, "schema.ts");
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

// playwright.local.config.ts starts the Kernel with LITTLEBOX_MAIL_TRANSPORT=development,
// which writes each message to mailbox.jsonl in the data directory instead of sending it.
async function invitationLink(to: string) {
  const mailbox = path.join(process.env.LITTLEBOX_E2E_DATA_DIR!, "mailbox.jsonl");
  let link: string | undefined;
  await expect(async () => {
    const lines = (await readFile(mailbox, "utf8").catch(() => "")).trim().split("\n").filter(Boolean);
    link = lines.map((line) => JSON.parse(line)).find((message) => message.purpose === "invitation" && message.to === to)?.link;
    expect(link).toBeTruthy();
  }).toPass({ timeout: 15_000 });
  return link!;
}

test("a private Invitation only app admits the person the owner invites from the Users view", async ({ page, context }, testInfo) => {
  const name = `Invited notes ${Date.now()}`;
  await createProject(page, name, "notes");
  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  // The radio itself is visually hidden; its card is what a person clicks.
  await page.getByRole("radiogroup", { name: "Deployment visibility" }).getByText("Private", { exact: true }).click();
  await expect(page.getByRole("radio", { name: /^Private/ })).toBeChecked();
  // A private first publish proposes Invitation only and says what it means.
  const signup = page.getByLabel("Who can create an app account");
  await expect(signup).toHaveValue("invitation");
  await expect(page.getByText("Only people you invite by email can create an app account", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Deploy private", exact: true }).click();
  const link = page.locator("a.published-url");
  await expect(link).toBeVisible({ timeout: 30_000 });
  const url = (await link.getAttribute("href"))!;
  await expect(page.getByText("New app accounts: Invitation only.")).toBeVisible();
  await page.getByRole("button", { name: "Invite people", exact: true }).click();

  // An uninvited visitor finds no way to create an app account.
  const app = await context.newPage();
  await app.goto(url);
  await expect(app.getByRole("heading", { name: "Sign in to this app" })).toBeVisible();
  await expect(app.getByRole("button", { name: "Create account" })).toHaveCount(0);

  const users = page.getByRole("region", { name: "App users" });
  await expect(users.getByLabel("Who can create an app account")).toHaveValue("invitation");
  const invitee = `guest-${crypto.randomUUID()}@example.test`;
  await users.getByLabel("Email addresses").fill(invitee);
  await users.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(users.getByRole("list", { name: "Invitation results" })).toContainText("Invitation email handed to mail.");
  const invitations = users.getByRole("group", { name: "Invitations" });
  const row = invitations.getByRole("row").filter({ hasText: invitee });
  await expect(row).toContainText("Pending");
  await expect(row).toContainText("Handed to mail");
  await expect(row).toContainText(/Expires /);
  await expect(users.getByRole("group", { name: "App accounts" }).getByRole("row").filter({ hasText: invitee })).toContainText("Pending");

  // The person opens the emailed link, chooses a password and enters.
  await app.goto(await invitationLink(invitee));
  await expect(app.getByRole("heading", { name: `Join ${name}` })).toBeVisible();
  await app.getByLabel("Choose a password").fill("Invited-password-2026");
  await app.getByRole("button", { name: "Accept and enter" }).click();
  await expect(app.getByPlaceholder("Keep a thought…")).toBeVisible();
  await app.getByRole("button", { name: "Sign out", exact: true }).click();
  await app.goto(new URL("/_littlebox/login", url).href);
  await app.getByLabel("Email").fill(invitee);
  await app.getByLabel("Password").fill("Invited-password-2026");
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(app.getByPlaceholder("Keep a thought…")).toBeVisible();

  await invitations.getByRole("button", { name: "Refresh invitations" }).click();
  await expect(row).toContainText("Accepted");
  await expect(row.getByRole("button", { name: /Resend/ })).toHaveCount(0);
  await expect(users.getByRole("group", { name: "App accounts" }).getByRole("row").filter({ hasText: invitee })).toContainText("Active");

  // Open as owner: the owner's own app account, created with the role owner, opens in a new tab.
  const ownerTab = context.waitForEvent("page");
  await users.getByRole("button", { name: "Open as owner", exact: true }).click();
  const owned = await ownerTab;
  await expect(owned.getByPlaceholder("Keep a thought…")).toBeVisible({ timeout: 15_000 });
  expect(await owned.evaluate(async () => (await (window as unknown as { miniapp: { auth: { session(): Promise<{ role: string }> } } }).miniapp.auth.session()).role)).toBe("owner");
  await expect(users.getByRole("group", { name: "Your account in this app" })).toContainText("with the role owner");
  await expect(users.getByRole("group", { name: "App accounts" }).getByRole("row").filter({ hasText: "Owner" })).toContainText("Active");
  await owned.close();
  // The Auth panel scrolls inside the window, so a tall window shows the whole view.
  // Buttons fade their colour on a theme change; "disabled" finishes that before the capture.
  await page.setViewportSize({ width: 1280, height: 2000 });
  await page.evaluate(() => window.scrollTo(0, 0));
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: testInfo.outputPath(`users-${scheme}.png`), animations: "disabled" });
  }

  // Integrations: a signing secret by name only, and an API key shown once with a working curl.
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByRole("navigation", { name: "Auth views" }).getByRole("button", { name: "Integrations" }).click();
  const integrations = page.getByRole("region", { name: "Integrations" });
  await integrations.getByLabel("Secret name").fill("orders_webhook");
  await integrations.getByLabel("Secret value").fill("a-long-enough-secret-2026");
  await integrations.getByRole("button", { name: "Save secret" }).click();
  await expect(integrations.getByRole("list", { name: "Signing secrets" })).toContainText("orders_webhook");
  await expect(integrations).not.toContainText("a-long-enough-secret-2026");
  await expect(integrations.getByLabel("Secret value")).toHaveValue("");
  await expect(integrations.getByLabel("Secret name")).toHaveValue("");
  await integrations.getByLabel("Key name").fill("Nightly backup");
  await integrations.getByLabel("Private files").check();
  await integrations.getByRole("button", { name: "Create API key" }).click();
  const reveal = page.getByRole("dialog", { name: "Copy this API key now" });
  await expect(reveal.locator("code").first()).toHaveText(/^lbk_/);
  await expect(reveal).toContainText(`${url.replace(/\/$/, "")}/_littlebox/files/`);
  await reveal.getByRole("button", { name: "I copied the key" }).click();
  await expect(integrations.getByRole("list", { name: "Live API keys" })).toContainText("Nightly backup");
  await expect(integrations.getByRole("list", { name: "Live API keys" })).toContainText("Bearer $LITTLEBOX_API_KEY");
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: testInfo.outputPath(`integrations-${scheme}.png`), animations: "disabled" });
  }
});

test("a person who writes code adds a page, a public file and a signed route, and the editor says what refused the publish", async ({ page }) => {
  await createProject(page, `Pages and routes ${Date.now()}`);
  const explorer = await openExplorer(page);
  for (const [file, contents] of [
    ["pages/about.html", '<main id=app><h1>About us</h1><img src="/logo.svg" alt="Logo"></main>'],
    ["public/logo.svg", '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>'],
    ["404.html", "<main id=app><h1>Nothing at this address</h1></main>"],
  ] as const) {
    await explorer.getByRole("button", { name: "New file" }).click();
    await page.getByLabel("File path").fill(file);
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await page.getByLabel(`${file} code editor`).fill(contents);
  }

  const actions = await openFile(page, "actions.json");
  const program = JSON.parse(await actions.inputValue());
  program.actions["tasks.toggle"].guard = "That task is gone.";
  program.actions["orders.receive"] = {
    kind: "mutation", access: "anonymous", statement: "INSERT INTO tasks (title, done, created_at) VALUES (?1, 0, ?2)",
    bindings: [{ source: "input", key: "title", type: "string", maxLength: 200 }, { source: "kernel", field: "now" }],
    http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } },
  };
  await actions.fill(JSON.stringify(program, null, 2));
  // The readout above actions.json names the route as the action's only door and the guard's answer.
  await expect(page.getByText("orders.receive. Only its route, POST /hooks/orders, runs it, for a signed request or an API key.")).toBeVisible();
  await expect(page.getByText('Marks a row done. Anyone can run it. When no row changes it refuses with "That task is gone.".')).toBeVisible();
  await waitForKernelSave(page);

  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  await page.getByRole("button", { name: "Deploy public", exact: true }).click();
  const refusal = page.getByRole("alert").filter({ hasText: "Deployment failed" });
  await expect(refusal).toContainText("Action orders.receive signs with the secret orders_webhook, which is not set for this project.");
  await refusal.getByRole("button", { name: "Set it in Auth > Integrations" }).click();
  await expect(page.getByText("orders.receive signs with it. Publishing is refused until it is set.")).toBeVisible();
  await page.getByRole("button", { name: "Set", exact: true }).click();
  await page.getByLabel("Secret value").fill("whsec-0123456789abcdef");
  await page.getByRole("button", { name: "Save secret" }).click();
  await expect(page.getByText("Used by orders.receive.")).toBeVisible();

  const url = await publish(page);
  const about = await page.request.get(`${url}/about`);
  expect(about.status()).toBe(200);
  expect(await about.text()).toContain("About us");
  const logo = await page.request.get(`${url}/logo.svg`);
  expect(logo.headers()["content-type"]).toContain("image/svg+xml");
  const missing = await page.request.get(`${url}/tasks/3`);
  expect(missing.status()).toBe(404);
  expect(await missing.text()).toContain("Nothing at this address");
});
