import { readFile } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "playwright/test";

test.beforeEach(async ({ page }) => {
  // The Kernel limits owner signups per client address. The test server trusts the local
  // proxy, so give each test its own forwarded address and repeated runs stay under the limit.
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await page.request.post("/_konae/owner/signup", { headers: { "x-forwarded-for": client }, data: { email: `owner-${crypto.randomUUID()}@example.test`, password: "Owner-test-password-2026", setupKey: "local-development-key" } });
  expect(response.ok()).toBe(true);
});

test("platform pages require an owner session and return to the requested page after sign-in", async ({ page }) => {
  const email = `owner-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const client = `203.0.113.${1 + Math.floor(Math.random() * 254)}`;
  const signup = await page.request.post("/_konae/owner/signup", { headers: { "x-forwarded-for": client }, data: { email, password, setupKey: "local-development-key" } });
  expect(signup.ok()).toBe(true);
  // A fresh owner has no projects, so a link to the browser starter would fall back to the
  // project list. Create a project first so the requested page is a real workspace.
  const projectPath = await createProject(page, `Return here ${Date.now()}`);
  await page.request.post("/_konae/owner/logout");

  // The component gallery is public (0b179e2), so only the workspace pages ask for sign-in.
  for (const path of ["/", projectPath]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/_konae\/owner\/login\?returnTo=/);
    await expect(page.getByRole("heading", { name: "Owner sign-in" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Workspace sections" })).toHaveCount(0);
  }

  await page.goto(projectPath);
  // The owner page also holds the hidden reset and new-password forms.
  const signIn = page.locator("form#auth");
  await signIn.getByLabel("Email").fill(email);
  await signIn.getByLabel("Password").fill(password);
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
  await page.goto("/");
  await page.getByRole("button", { name: "New project" }).click();
  await page.getByRole("textbox", { name: "Project name", exact: true }).fill(name);
  await page.getByText("Start from a template instead", { exact: true }).click();
  await page.getByRole("radio", { name: new RegExp(`^${templates[template]}`) }).check();
  const saved = page.waitForResponse((response) => {
    const body = response.request().postData() ?? "";
    return new URL(response.url()).pathname === "/mcp" && response.ok()
      && (body.includes('"project_sources_save"') || (body.includes('"project_create"') && body.includes('"sourcesTransferId"')));
  });
  await page.getByRole("button", { name: /^Create/ }).click();
  await expect(page).toHaveURL(/\/app\/[^/]+$/);
  await expect(page.getByRole("banner").locator(".project-crumb-current").getByText(name, { exact: true })).toBeVisible();
  await saved;
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0);
  await expect(page.getByRole("banner").getByText("Not saved")).toHaveCount(0);
  return new URL(page.url()).pathname;
}

// The editor sends project sources to the Kernel 500 ms after the last edit (App.tsx) and
// shows "Saving" until the Kernel has them. A reload before that loads the older sources.
async function waitForKernelSave(page: Page) {
  await page.waitForTimeout(1_000);
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByRole("banner").getByText("Not saved")).toHaveCount(0);
}

async function showSection(page: Page, name: "Preview" | "Access" | "Files" | "Data" | "Uploads") {
  await page.getByRole("navigation", { name: "Workspace sections" }).getByRole("tab", { name, exact: true }).click();
}

// Export and App settings live in the top bar's More menu.
async function fromMore(page: Page, item: "Export" | "App settings") {
  await page.getByRole("banner").getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
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
  const picker = page.getByLabel("Open file");
  if (await picker.isVisible()) {
    await picker.selectOption(path);
  } else {
    const explorer = page.getByRole("navigation", { name: "Project files" });
    if (!(await explorer.isVisible())) await page.getByRole("button", { name: "File list", exact: true }).click();
    await explorer.getByRole("button", { name: path, exact: true }).click();
  }
  return page.getByLabel(`${path} code editor`);
}

// The top bar's button reads "Deploy" on a draft and "Deploy changes" once the app is published.
async function publish(page: Page) {
  await page.getByRole("banner").getByRole("button", { name: /^Deploy( changes)?$/ }).click();
  // A first publish of a private template starts on Private; these tests publish public apps.
  const visibility = page.getByRole("radiogroup", { name: "Deployment visibility" });
  if (await visibility.count()) await visibility.getByText("Public", { exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: /^Deploy (public|changes)$/ }).click();
  const link = page.locator("a.published-url");
  await expect(link).toBeVisible({ timeout: 30_000 });
  const url = await link.getAttribute("href");
  await page.getByRole("button", { name: "Done", exact: true }).click();
  return url!;
}

test("workspace count follows Kernel projects across the component gallery", async ({ page }) => {
  for (const name of ["Count one", "Count two", "Count three"]) await createProject(page, name);
  await page.goto("/");
  const projects = page.locator(".account-sidebar nav a[href='/']").first();
  await expect(projects).toHaveText("Projects 3");
  const drafts = page.getByRole("region", { name: "Drafts" });
  await expect(drafts.getByRole("listitem")).toHaveCount(3);
  await expect(drafts.locator(".app-status-address")).toHaveCount(0);
  await page.goto("/design-system");
  await expect(page.locator(".account-sidebar nav a[href='/']").first()).toHaveText("Projects 3");
  await page.goto("/usage");
  await expect(page.locator(".account-sidebar nav a[href='/']").first()).toHaveText("Projects 3");
  await expect(page.locator(".account-content").getByRole("heading", { name: "Usage", exact: true })).toBeVisible();
});

test("uploads added through the app appear in the editor file list", async ({ page }) => {
  await createProject(page, "Preview files");
  await showSection(page, "Preview");
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  // Guest first: the app must sign in and still keep the chosen file.
  await preview.locator('input[type="file"]').setInputFiles({ name: "elena furia.png", mimeType: "image/png", buffer: Buffer.from("fake-png-bytes") });
  await expect(preview.getByRole("img", { name: "elena furia.png" })).toBeVisible();
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await showSection(page, "Uploads");
  const uploads = page.getByRole("region", { name: "Uploads", exact: true });
  await expect(uploads.getByText("elena furia.png", { exact: true })).toBeVisible();
});

test("mobile domain settings persist claims, explain TXT checks and confirm changes", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.setViewportSize({ width: 320, height: 740 });
  await page.emulateMedia({ colorScheme: "dark" });
  const projectPath = await createProject(page, "Mobile domain review");
  await fromMore(page, "App settings");
  let settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await expect(settings.getByText("Deploy this app first. Then its addresses appear here.", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "Close App settings", exact: true }).click();
  await publish(page);
  await fromMore(page, "App settings");
  settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await settings.getByLabel("Domain", { exact: true }).fill("https://notes.example.test/path");
  await settings.getByRole("button", { name: "Add domain", exact: true }).click();
  await expect(settings.getByLabel("Domain", { exact: true })).toHaveAttribute("aria-invalid", "true");
  await expect(settings.getByText("Enter only the hostname, without https:// or a path.", { exact: true })).toBeVisible();
  await expect(settings.getByRole("button", { name: "Reload domain settings", exact: true })).toHaveCount(0);
  await settings.getByLabel("Domain", { exact: true }).fill("notes.example.test");
  await settings.getByRole("button", { name: "Add domain", exact: true }).click();
  await expect(settings.getByText("Waiting for TXT", { exact: true })).toBeVisible();
  await expect(settings.getByText("_konae.notes.example.test", { exact: true })).toBeVisible();
  await expect(settings.getByRole("button", { name: "Copy TXT value", exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "Copy TXT name", exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("_konae.notes.example.test");
  const token = await settings.locator(".domain-record code").nth(1).textContent();
  await settings.getByRole("button", { name: "Copy TXT value", exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(token);
  await expect(settings.getByText("Not checked", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "Check TXT record", exact: true }).click();
  await expect(settings.getByText("Ownership is still pending.", { exact: false })).toBeVisible();
  await page.goto(projectPath);
  await fromMore(page, "App settings");
  settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await expect(settings.getByLabel("Domain", { exact: true })).toHaveValue("notes.example.test");
  await settings.getByLabel("Domain", { exact: true }).fill("new-notes.example.test");
  await settings.getByRole("button", { name: "Change domain", exact: true }).click();
  await expect(settings.getByRole("region", { name: "Confirm domain change", exact: true })).toContainText("even if the new TXT is still pending");
  await settings.getByRole("button", { name: "Cancel change", exact: true }).click();
  await expect(settings.getByText("_konae.notes.example.test", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "Change domain", exact: true }).click();
  await settings.getByRole("button", { name: "Replace domain", exact: true }).click();
  await expect(settings.getByText("_konae.new-notes.example.test", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "Remove domain", exact: true }).click();
  await settings.getByRole("button", { name: "Confirm removal", exact: true }).click();
  await expect(settings.getByText("No custom domain", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "light" });
  const bounds = await settings.boundingBox();
  expect(bounds?.width).toBeLessThanOrEqual(390);
});

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
  await app.goto(new URL("/_konae/login", url!).href);
  await signInApp(app);
  await expect(app.getByText("Survives a new release")).toBeVisible();
  // The SDK hands out the Kernel's file path on the app's own origin, which connect-src allows.
  // That path must hand the HTML back as a download, never a page.
  const attachment = await app.evaluate(async () => {
    const sdk = (window as unknown as { miniapp: { files: { upload(file: File): Promise<{ id: string }> } } }).miniapp;
    const stored = await sdk.files.upload(new File(['<script>document.body.dataset.executed="yes"</script>'], "untrusted.html", { type: "text/html" }));
    const response = await fetch(`/_konae/files/${stored.id}`);
    return { status: response.status, disposition: response.headers.get("content-disposition"), sniffing: response.headers.get("x-content-type-options") };
  });
  expect(attachment).toEqual({ status: 200, disposition: "attachment", sniffing: "nosniff" });
  expect(await publish(page)).toBe(url);
  await app.reload();
  await expect(app.getByText("Survives a new release")).toBeVisible();

  await showSection(page, "Data");
  await expect(page.getByRole("radio", { name: "Live app", exact: true })).toBeChecked();
  await page.getByRole("tablist", { name: "Database views" }).getByRole("tab", { name: "Backups" }).click();
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
  await fromMore(page, "Export");
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toMatch(/\.tar$/);
  await download.saveAs(testInfo.outputPath("app-folder.tar"));
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
  await showSection(page, "Access");
  // The list rule agrees with the template: only a signed-in person lists notes.
  await expect(page.getByText("A signed-in person can run it. The list stops at 100 rows.")).toBeVisible();
  await page.getByLabel("Preview user").selectOption("bob");
  await showSection(page, "Preview");
  await expect(preview.getByText("No notes yet. Start with one thought.")).toBeVisible();
  await expect(preview.getByText("Alice preview note")).toHaveCount(0);
  await showSection(page, "Access");
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
  await fromMore(page, "Export");
  const download = await downloadEvent;
  await download.saveAs(testInfo.outputPath("notes-app-folder.tar"));
});

// playwright.local.config.ts starts the Kernel with KONAE_MAIL_TRANSPORT=development,
// which writes each message to mailbox.jsonl in the data directory instead of sending it.
async function invitationLink(to: string) {
  const mailbox = path.join(process.env.KONAE_E2E_DATA_DIR!, "mailbox.jsonl");
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
  await app.goto(new URL("/_konae/login", url).href);
  await app.getByLabel("Email").fill(invitee);
  await app.getByLabel("Password").fill("Invited-password-2026");
  await app.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(app.getByPlaceholder("Keep a thought…")).toBeVisible();

  await invitations.getByRole("button", { name: "Refresh invitations" }).click();
  await expect(row).toContainText("Accepted");
  await expect(row.getByRole("button", { name: /Resend/ })).toHaveCount(0);
  await expect(users.getByRole("group", { name: "App accounts" }).getByRole("row").filter({ hasText: invitee })).toContainText("Active");

  // The Access panel scrolls inside the window, so a tall window shows the whole view.
  // Buttons fade their colour on a theme change; "disabled" finishes that before the capture.
  await page.setViewportSize({ width: 1280, height: 2000 });
  await page.evaluate(() => window.scrollTo(0, 0));
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: testInfo.outputPath(`users-${scheme}.png`), animations: "disabled" });
  }

  // Integrations: a signing secret by name only, and an API key shown once with a working curl.
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByRole("tablist", { name: "Access views" }).getByRole("tab", { name: "Integrations" }).click();
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
  await expect(reveal).toContainText(`${url.replace(/\/$/, "")}/_konae/files/`);
  await reveal.getByRole("button", { name: "I copied the key" }).click();
  await expect(integrations.getByRole("list", { name: "Live API keys" })).toContainText("Nightly backup");
  await expect(integrations.getByRole("list", { name: "Live API keys" })).toContainText("Bearer $KONAE_API_KEY");
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
  await page.getByText("What each action does", { exact: true }).click();
  await expect(page.getByText("Receive. Only its route, POST /hooks/orders, runs it, for a signed request or an API key.")).toBeVisible();
  await expect(page.getByText('Mark a row done. Anyone can run it. When no row changes it refuses with "That task is gone.".')).toBeVisible();
  await waitForKernelSave(page);

  // The deploy dialog reads the release plan first, so it names the unset secret and keeps Deploy off.
  await page.getByRole("button", { name: "Deploy", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: /^Deploy / });
  await expect(dialog).toContainText("Action orders.receive signs with the secret orders_webhook, which is not set for this project.");
  await expect(dialog.getByRole("button", { name: "Deploy public", exact: true })).toBeDisabled();
  await dialog.getByRole("button", { name: "Open Access > Integrations" }).click();
  await expect(page.getByText("orders.receive uses it. Publishing is refused until it is set.")).toBeVisible();
  await page.getByRole("button", { name: "Set", exact: true }).click();
  await page.getByLabel("Secret value").fill("whsec-0123456789abcdef");
  await page.getByRole("button", { name: "Save secret" }).click();
  await expect(page.getByText(/^orders\.receive uses it\. Updated /)).toBeVisible();

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

test("usage counts signed-in accounts as people, never a visitor, and shows what each app stores", async ({ page, browser }, testInfo) => {
  const name = `Usage people ${Date.now()}`;
  await createProject(page, name);
  const url = await publish(page);
  const addTask = async (app: Page, text: string) => {
    await app.getByPlaceholder(/Add one small thing/).fill(text);
    await app.getByRole("button", { name: "Add task" }).click();
    await expect(app.getByText(text)).toBeVisible();
  };
  // A visitor without an account runs an action: an operation, never a person.
  const anonymous = await browser.newContext();
  const visitor = await anonymous.newPage();
  await visitor.goto(url);
  await addTask(visitor, "From a visitor");
  await anonymous.close();
  // Two people with accounts, one busier than the other.
  for (const tasks of [3, 1]) {
    const context = await browser.newContext();
    const person = await context.newPage();
    await person.goto(new URL("/_konae/login", url).href);
    await signInApp(person);
    for (let index = 0; index < tasks; index += 1) await addTask(person, `Task ${tasks}-${index}`);
    await context.close();
  }

  await page.goto("/usage");
  await expect(page.getByRole("heading", { name: "All apps" })).toBeVisible();
  const row = page.getByRole("row").filter({ has: page.getByRole("rowheader", { name: new RegExp(name) }) });
  await expect(row.locator('td[data-label="Active accounts"]')).toHaveText("2");
  await expect(row.locator('td[data-label="Storage"]')).toHaveText(/KiB|MiB/);
  await page.getByRole("region", { name: "All apps" }).screenshot({ path: testInfo.outputPath("usage-overview-light.png") });

  await row.getByRole("link", { name: `View usage for ${name}` }).click();
  const people = page.getByRole("region", { name: "People" });
  await expect(people.locator(".ui-stats")).toContainText("Active accounts2");
  // Actions count every call an account made, the list queries the page runs as well as the adds.
  const ranked = people.getByRole("figure").filter({ hasText: "Actions by account" });
  await expect(ranked.locator("tbody tr")).toHaveCount(2);
  const counts = (await ranked.locator("tbody .ui-barlist-value").allTextContents()).map(Number);
  expect(counts[0]).toBeGreaterThan(counts[1]);
  await expect(people.locator(".ui-stats")).toContainText(`Actions${counts[0] + counts[1]}`);
  await expect(ranked.getByText(/@example\.test/).first()).toBeVisible();
  await people.getByRole("radio", { name: "App opens" }).click();
  await expect(people.getByRole("figure").filter({ hasText: "App opens by account" })).toBeVisible();
  const storage = page.getByRole("region", { name: "Storage" });
  await expect(storage.getByRole("rowheader", { name: /^Database/ })).toBeVisible();
  await expect(storage.getByRole("rowheader", { name: /^Published pages/ })).toBeVisible();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await people.screenshot({ path: testInfo.outputPath(`usage-people-${scheme}.png`), animations: "disabled" });
    await storage.screenshot({ path: testInfo.outputPath(`usage-storage-${scheme}.png`), animations: "disabled" });
  }
});
