import { readFile } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "playwright/test";
import { McpClient, type Transfer } from "../../src/mcp/client";
import { publishProject } from "../../src/deployments/release";

test.beforeEach(async ({ page }) => {
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await page.request.post("/_konae/owner/signup", { headers: { "x-forwarded-for": client }, data: { email: `owner-${crypto.randomUUID()}@example.test`, password: "Owner-test-password-2026", setupKey: "local-development-key" } });
  expect(response.ok()).toBe(true);
});

function sourceReadRequest(body: string): { projectId?: string; requestId?: string; cancelRequestId?: string; metadataOnly?: boolean } | undefined {
  try {
    const request = JSON.parse(body);
    if (request.params?.name !== "project_sources_read") return undefined;
    return request.params.arguments;
  } catch { return undefined; }
}

function isFullSourceRead(body: string): boolean {
  const args = sourceReadRequest(body);
  return Boolean(args?.projectId && args.requestId && !args.cancelRequestId && args.metadataOnly !== true);
}

test("platform pages require an owner session and return to the requested page after sign-in", async ({ page }) => {
  const email = `owner-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const client = `203.0.113.${1 + Math.floor(Math.random() * 254)}`;
  const signup = await page.request.post("/_konae/owner/signup", { headers: { "x-forwarded-for": client }, data: { email, password, setupKey: "local-development-key" } });
  expect(signup.ok()).toBe(true);
  const projectPath = await createProject(page, `Return here ${Date.now()}`);
  await page.request.post("/_konae/owner/logout");

  for (const path of ["/", projectPath]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/_konae\/owner\/login\?returnTo=/);
    await expect(page.getByRole("heading", { name: "Owner sign-in" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Workspace sections" })).toHaveCount(0);
  }

  await page.goto(projectPath);
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

async function writeTypedPreview(page: Page, message: string) {
  const explorer = await openExplorer(page);
  await explorer.getByRole("button", { name: "New file" }).click();
  await page.getByLabel("File path").fill("src/greeting.ts");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.getByLabel("src/greeting.ts code editor").fill(`export const greeting: string = ${JSON.stringify(message)};`);
  await explorer.getByRole("button", { name: "app.js" }).click();
  await page.getByLabel("app.js code editor").fill('import { greeting } from "./src/greeting.ts"; document.body.innerHTML = `<h1>${greeting}</h1>`;');
  await waitForKernelSave(page);
}

async function seedRetainedBinary(page: Page, projectId: string) {
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", page.url()).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const inspected = await client.call<{ sourcesRevision: number }>("project_inspect", { projectId });
  const saved = await client.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const source = JSON.parse(new TextDecoder().decode(await client.download(saved.transfer))) as { files: Record<string, string>; encodings?: Record<string, "base64"> };
  const asset = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/pWQAAAAASUVORK5CYII=";
  const appliedImage = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGO4I6fxHxkzkC4AAIMIIhELyVVQAAAAAElFTkSuQmCC";
  const candidateImage = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGMQCbjzHxkzkC4AAKv4I/ELTj/5AAAAAElFTkSuQmCC";
  source.files["public/retained.png"] = asset;
  source.files["public/applied.png"] = appliedImage;
  source.files["public/candidate.png"] = candidateImage;
  source.files["public/theme.css"] = '.asset-stylesheet-probe { color: rgb(16, 32, 48); background-image: url("./applied.png?from=theme#background"); }';
  source.files["public/print.css"] = '.media-print-probe { color: rgb(16, 32, 48); }';
  source.files["public/disabled.css"] = '.media-disabled-probe { background-color: rgb(16, 32, 48); }';
  source.files["index.html"] += '<link rel="stylesheet" href="/theme.css"><link rel="stylesheet" href="/print.css" media="print"><link rel="stylesheet" href="/disabled.css" media="screen" disabled><p id="stylesheet-probe" class="asset-stylesheet-probe">Stylesheet proof</p><p id="media-default-probe">Default screen style</p><p class="media-print-probe">Print only</p><p class="media-disabled-probe">Initially disabled</p><img id="asset-image" alt="Applied public asset" width="48" height="48" src="/applied.png?version=applied#preview">';
  source.encodings = { ...source.encodings, "public/retained.png": "base64", "public/applied.png": "base64", "public/candidate.png": "base64" };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(source)), projectId);
  await client.call("project_sources_save", { projectId, transferId: transfer.transferId, expectedRevision: inspected.sourcesRevision });
  return asset;
}

async function readSavedSources(page: Page, projectId: string) {
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", page.url()).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const saved = await client.call<{ transfer: Transfer }>("project_sources_read", { projectId });
  return JSON.parse(new TextDecoder().decode(await client.download(saved.transfer))) as { files: Record<string, string>; encodings?: Record<string, "base64"> };
}

async function waitForKernelSave(page: Page) {
  await page.waitForTimeout(1_000);
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByRole("banner").getByText("Not saved")).toHaveCount(0);
}

async function showSection(page: Page, name: "Preview" | "Describe" | "Access" | "Files" | "Data" | "Uploads") {
  const tab = page.getByRole("navigation", { name: "Workspace sections" }).getByRole("tab", { name, exact: true });
  if (await tab.count()) {
    await tab.click();
    await expect(tab).toHaveAttribute("aria-selected", "true");
    return;
  }
  if (name === "Describe") {
    const open = page.getByRole("button", { name: "Open Describe", exact: true });
    if (await open.isVisible()) await open.click();
    return;
  }
  throw new Error(`The ${name} section is not available.`);
}

function providerEvent(type: string, data: unknown) {
  return `event: ${type}\ndata: ${JSON.stringify(data)}\n\n`;
}

function providerReply(tool?: { name: string; input: Record<string, unknown> }, text = "The checked change is ready.") {
  const contentBlock = tool
    ? { type: "tool_use", id: `toolu_${crypto.randomUUID()}`, name: tool.name, input: {} }
    : { type: "text", text: "" };
  const events = [providerEvent("message_start", {
    type: "message_start",
    message: { id: `msg_${crypto.randomUUID()}`, type: "message", role: "assistant", model: "claude-3-5-sonnet-latest", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 1 } },
  })];
  events.push(providerEvent("content_block_start", { type: "content_block_start", index: 0, content_block: contentBlock }));
  events.push(providerEvent("content_block_delta", tool
    ? { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(tool.input) } }
    : { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } }));
  events.push(providerEvent("content_block_stop", { type: "content_block_stop", index: 0 }));
  events.push(providerEvent("message_delta", { type: "message_delta", delta: { stop_reason: tool ? "tool_use" : "end_turn", stop_sequence: null }, usage: { output_tokens: 1 } }));
  events.push(providerEvent("message_stop", { type: "message_stop" }));
  return events.join("");
}

async function installProposalFixture(page: Page) {
  let scenario: "discard" | "apply" | "asset-discard" | "asset-discard-applied" | "asset-apply" | "blocked" | "drift" | "build-fail" | "preview-behind" = "discard";
  let step = 0;
  const html = "<main><h1>PROPOSAL PREVIEW</h1><p id='proposal-actor'></p><p id='proposal-rows'></p><p id='proposal-files'></p></main>";
  const assetHtml = (useApplied: boolean) => `<link rel='stylesheet' href='/theme.css'><link rel='stylesheet' href='/print.css' media='print'><link rel='stylesheet' href='/disabled.css' media='screen' disabled><main><h1>PROPOSAL PREVIEW</h1><p id='stylesheet-probe' class='asset-stylesheet-probe'>Stylesheet proof</p><p id='media-default-probe'>Default screen style</p><p class='media-print-probe'>Print only</p><p class='media-disabled-probe'>Initially disabled</p><img id='asset-image' alt='Candidate public asset' width='48' height='48' src='/${useApplied ? "applied.png?version=alternate" : "candidate.png?version=candidate"}#proposal'><p id='proposal-actor'></p><p id='proposal-rows'></p><p id='proposal-files'></p></main>`;
  const proposalScript = (mutate: boolean) => `
const rows = await miniapp.actions.call('tasks.list');
document.querySelector('#proposal-rows').textContent = rows.map(row => row.title).join(', ');
const session = await miniapp.auth.session();
document.querySelector('#proposal-actor').textContent = session.name;
const files = await miniapp.files.list();
document.querySelector('#proposal-files').textContent = files.map(file => file.name).join(', ');
${mutate ? "await miniapp.actions.call('tasks.create', { title: 'Scratch row' }); if (files[0]) await miniapp.files.delete(files[0].id);" : ""}
`;
  const invalidActions = JSON.stringify({ version: 1, actions: {
    "tasks.update": { kind: "mutation", access: "anonymous", statement: "UPDATE tasks SET done=1 WHERE id=1", bindings: [], notify: { to: "owner", changes: ["missing"], subject: "Changed", body: "{changes}" } },
    "tasks.list": { kind: "query", access: "anonymous", statement: "SELECT missing FROM tasks LIMIT 5", bindings: [] },
  } });
  await page.route("https://api.anthropic.com/**", async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-methods": "POST,OPTIONS", "access-control-allow-headers": "*" } });
      return;
    }
    let tool: { name: string; input: Record<string, unknown> } | undefined;
    if (scenario === "blocked") {
      if (step === 0) tool = { name: "write", input: { path: "/workspace/actions.json", content: invalidActions } };
      else if (step === 1) tool = { name: "validate_capsule", input: {} };
    } else if (step === 0) {
      const path = scenario === "preview-behind" ? "/workspace/schema.sql" : "/workspace/index.html";
      const content = scenario === "preview-behind" ? `
CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  done INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  due_at INTEGER
);
` : scenario === "asset-discard" || scenario === "asset-discard-applied" || scenario === "asset-apply" ? assetHtml(scenario === "asset-discard-applied") : html;
      tool = { name: "write", input: { path, content } };
    } else if (step === 1) {
      tool = { name: "write", input: { path: "/workspace/app.js", content: scenario === "discard" || scenario === "apply" || scenario === "asset-discard" || scenario === "asset-discard-applied" || scenario === "asset-apply" ? proposalScript(scenario === "discard" || scenario === "asset-discard" || scenario === "asset-discard-applied") : scenario === "build-fail" ? "const = ;" : "document.documentElement.dataset.proposal = 'ready';" } };
    } else if (step === 2) {
      tool = { name: "validate_capsule", input: {} };
    }
    step += 1;
    if (!tool) step = 0;
    const origin = route.request().headers().origin ?? "*";
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      headers: { "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-expose-headers": "*", "cache-control": "no-cache" },
      body: providerReply(tool),
    });
  });
  return { setScenario(value: typeof scenario) { scenario = value; } };
}

async function installModuleSetFixture(page: Page) {
  let step = 0;
  await page.route("https://api.anthropic.com/**", async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-methods": "POST,OPTIONS", "access-control-allow-headers": "*" } });
      return;
    }
    const response = step++ === 0
      ? providerReply({ name: "module_set", input: { module: "seo", on: true, options: { canonical: "path" }, expected: { on: false, canonical: null, image: null } } })
      : providerReply(undefined, "I read the module state after the change.");
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-headers": "*", "access-control-expose-headers": "*", "cache-control": "no-cache" },
      body: response,
    });
  });
}

async function connectProposalFixture(page: Page) {
  await showSection(page, "Describe");
  await page.getByRole("button", { name: "Connect a model to send" }).click();
  const dialog = page.getByRole("dialog", { name: "Connect a model" });
  await dialog.getByLabel("Anthropic API key").fill("fixture-key-unused");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toHaveCount(0);
}

async function askFixture(page: Page, prompt: string) {
  await page.getByRole("textbox", { name: "Message to agent" }).fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
}

async function fromMore(page: Page, item: "Export" | "App settings") {
  await page.getByRole("banner").getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

async function openExplorer(page: Page) {
  await showSection(page, "Files");
  const explorer = page.getByRole("navigation", { name: "Project files" });
  if (!await explorer.isVisible()) await page.getByRole("button", { name: "File list", exact: true }).click();
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

async function publish(page: Page) {
  await page.getByRole("banner").getByRole("button", { name: /^Deploy( changes)?$/ }).click();
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
  await preview.locator('input[type="file"]').setInputFiles({ name: "elena furia.png", mimeType: "image/png", buffer: Buffer.from("fake-png-bytes") });
  await expect(preview.getByRole("img", { name: "elena furia.png" })).toBeVisible();
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await showSection(page, "Uploads");
  const uploads = page.getByRole("region", { name: "Uploads", exact: true });
  await expect(uploads.getByText("elena furia.png", { exact: true })).toBeVisible();
});

test("a Gate-approved proposal previews before Apply and isolates discarded rows", async ({ page }) => {
  const projectPath = await createProject(page, "Proposal preview", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const retainedAsset = await seedRetainedBinary(page, projectId);
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Preview");
  let preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await preview.locator("#task-input").fill("Saved row");
  await preview.getByRole("button", { name: "Add task" }).click();
  await expect(preview.getByText("Saved row", { exact: true })).toBeVisible();
  await preview.locator('input[type="file"]').setInputFiles({ name: "proposal-proof.png", mimeType: "image/png", buffer: Buffer.from("preview-image-bytes") });
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await expect(preview.getByRole("img", { name: "proposal-proof.png" })).toBeVisible();

  const fixture = await installProposalFixture(page);
  await connectProposalFixture(page);
  fixture.setScenario("discard");
  await askFixture(page, "Discard this preview proposal after checking it.");
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 30_000 });
  let proposalFrame = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(proposalFrame.locator("#proposal-rows")).toHaveText("Saved row");
  await expect(proposalFrame.locator("#proposal-actor")).toHaveText("Alice");
  await expect(proposalFrame.locator("#proposal-files")).toHaveText("proposal-proof.png");
  await showSection(page, "Describe");
  await expect(page.getByText("Validated. Sources, schema, preview rows and actions hold.")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  await showSection(page, "Preview");
  await page.screenshot({ path: "/tmp/konae-u3-proposal/proposal-before-apply.png" });

  const original = await openFile(page, "index.html");
  expect(await original.inputValue()).not.toContain("PROPOSAL PREVIEW");
  await showSection(page, "Preview");
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible();
  await showSection(page, "Describe");
  await page.getByRole("button", { name: "Discard", exact: true }).click();
  await showSection(page, "Preview");
  preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("Saved row", { exact: true })).toBeVisible();
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await expect(preview.getByRole("img", { name: "proposal-proof.png" })).toBeVisible();

  fixture.setScenario("apply");
  await showSection(page, "Describe");
  await askFixture(page, "Show the checked proposal and apply it.");
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 30_000 });
  proposalFrame = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(proposalFrame.locator("#proposal-rows")).toHaveText("Saved row");
  await expect(proposalFrame.locator("#proposal-actor")).toHaveText("Alice");
  await expect(proposalFrame.locator("#proposal-files")).toHaveText("proposal-proof.png");
  await expect(page.getByText("Validated. Sources, schema, preview rows and actions hold.")).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await showSection(page, "Preview");
  proposalFrame = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(proposalFrame.getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible();
  await expect(proposalFrame.locator("#proposal-rows")).toHaveText("Saved row");
  await expect(proposalFrame.locator("#proposal-actor")).toHaveText("Alice");
  await expect(proposalFrame.locator("#proposal-files")).toHaveText("proposal-proof.png");
  await openFile(page, "index.html");
  expect(await page.getByLabel("index.html code editor").inputValue()).toContain("PROPOSAL PREVIEW");
  await waitForKernelSave(page);
  const savedSources = await readSavedSources(page, projectId);
  expect(savedSources.files["public/retained.png"]).toBe(retainedAsset);
  expect(savedSources.encodings?.["public/retained.png"]).toBe("base64");

  fixture.setScenario("blocked");
  await showSection(page, "Describe");
  await askFixture(page, "Give me two invalid row-reading actions.");
  await expect(page.getByText("Check code: notify.column-unknown")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText("Try this. Name columns of the table the statement changes, in notify \"changes\" and \"to\".")).toBeVisible();
  await expect(page.getByText("3 more problems need fixing.")).toBeVisible();
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "BLOCKED PROPOSAL" })).toHaveCount(0);
  await page.getByRole("button", { name: "Discard", exact: true }).click();

  fixture.setScenario("preview-behind");
  await showSection(page, "Describe");
  await askFixture(page, "Show the schema change and explain what the preview could not check.");
  await expect(page.getByText("Validated. Sources, schema and actions hold. The preview rows were not checked.", { exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText(/The preview's test database is behind schema\.sql/)).toBeVisible();
  await expect(page.getByText("Its table tasks has no due_at.", { exact: true })).toBeVisible();
  await expect(page.getByText(/try the change on konae dev or a published Deployment\./i)).toBeVisible();
  await page.getByRole("button", { name: "Discard", exact: true }).click();

  fixture.setScenario("build-fail");
  await showSection(page, "Describe");
  await askFixture(page, "Try an invalid script so preview preparation fails.");
  const applyButton = page.locator(".agent-proposal").getByRole("button", { name: "Apply", exact: true });
  await expect(applyButton).toBeDisabled({ timeout: 30_000 });
  await expect(page.getByText(/The proposal preview could not be prepared\./)).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: "Discard", exact: true }).click();

  fixture.setScenario("drift");
  await askFixture(page, "Make a proposal, then keep my file edit.");
  await expect(page.locator(".agent-proposal").getByRole("button", { name: "Apply", exact: true })).toBeEnabled({ timeout: 30_000 });
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 30_000 });
  const styles = await openFile(page, "styles.css");
  await expect(styles).toBeVisible();
  await styles.fill(`${await styles.inputValue()}\n/* Manual owner edit */`);
  await showSection(page, "Describe");
  await expect(page.locator(".agent-proposal").getByRole("button", { name: "Apply", exact: true })).toHaveCount(0);
  await openFile(page, "styles.css");
  expect(await page.getByLabel("styles.css code editor").inputValue()).toContain("Manual owner edit");
  await waitForKernelSave(page);
});

test("public image bytes stay with each proposal through Discard and Apply", async ({ page }) => {
  await page.addInitScript(() => {
    const active = new Set<string>();
    (window as typeof window & { __konaeObjectUrls?: Set<string> }).__konaeObjectUrls = active;
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (value: Blob | MediaSource) => {
      const url = create(value);
      active.add(url);
      return url;
    };
    URL.revokeObjectURL = (url: string) => {
      active.delete(url);
      revoke(url);
    };
  });
  const projectPath = await createProject(page, "Proposal public assets", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const retainedAsset = await seedRetainedBinary(page, projectId);
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Preview");
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  const appliedImage = preview.locator("#asset-image");
  await expect.poll(() => appliedImage.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(4);
  await expect.poll(() => preview.locator("#stylesheet-probe").evaluate((node) => getComputedStyle(node).color)).toBe("rgb(16, 32, 48)");
  expect(await preview.locator("#stylesheet-probe").evaluate((node) => getComputedStyle(node).backgroundImage)).toContain("data:image/png;base64,");
  const screenMedia = await preview.locator("#media-default-probe").evaluate((node) => {
    const defaultStyle = getComputedStyle(node);
    const printStyle = getComputedStyle(document.querySelector(".media-print-probe")!);
    const disabledStyle = getComputedStyle(document.querySelector(".media-disabled-probe")!);
    const disabledSheet = [...document.styleSheets].find((sheet) => (sheet.ownerNode as HTMLStyleElement).dataset.konaeMedia !== undefined);
    return { defaultColor: defaultStyle.color, printColor: printStyle.color, defaultBackground: defaultStyle.backgroundColor, disabledBackground: disabledStyle.backgroundColor, disabled: disabledSheet?.disabled, disabledMedia: (disabledSheet?.ownerNode as HTMLStyleElement | undefined)?.media, printMedia: [...document.querySelectorAll("style")].find((style) => style.media === "print")?.media };
  });
  expect(screenMedia.printColor).toBe(screenMedia.defaultColor);
  expect(screenMedia.disabledBackground).toBe(screenMedia.defaultBackground);
  expect(screenMedia.disabled).toBe(true);
  expect(screenMedia.disabledMedia).toBe("screen");
  expect(screenMedia.printMedia).toBe("print");
  const appliedDataUrl = await appliedImage.getAttribute("src");
  expect(appliedDataUrl).toContain("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+");

  await preview.locator("#task-input").fill("Saved row");
  await preview.getByRole("button", { name: "Add task" }).click();
  await expect(preview.getByText("Saved row", { exact: true })).toBeVisible();
  await preview.locator('input[type="file"]').setInputFiles({ name: "proposal-proof.png", mimeType: "image/png", buffer: Buffer.from("preview-image-bytes") });
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await expect(preview.getByRole("img", { name: "proposal-proof.png" })).toBeVisible();

  const fixture = await installProposalFixture(page);
  await connectProposalFixture(page);
  const storageBefore = await page.evaluate(() => JSON.stringify(Object.entries(localStorage).sort(([a], [b]) => a.localeCompare(b))));
  const objectUrlsIn = async () => {
    const frame = page.frames().find((candidate) => candidate.parentFrame() !== null);
    return frame?.evaluate(() => (window as typeof window & { __konaeObjectUrls?: Set<string> }).__konaeObjectUrls?.size ?? 0) ?? 0;
  };

  for (let iteration = 0; iteration < 3; iteration += 1) {
    const useAlternateImage = iteration % 2 === 1;
    fixture.setScenario(useAlternateImage ? "asset-discard-applied" : "asset-discard");
    await showSection(page, "Describe");
    await askFixture(page, `Preview the candidate image ${iteration + 1}.`);
    await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 30_000 });
    const candidate = page.frameLocator('iframe[title="Mini-app preview"]').locator("#asset-image");
    await expect.poll(() => candidate.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(4);
    await expect.poll(() => page.frameLocator('iframe[title="Mini-app preview"]').locator("#stylesheet-probe").evaluate((node) => getComputedStyle(node).color)).toBe("rgb(16, 32, 48)");
    const candidateMedia = await page.frameLocator('iframe[title="Mini-app preview"]').locator("#media-default-probe").evaluate((node) => {
      const defaultStyle = getComputedStyle(node);
      return { defaultColor: defaultStyle.color, printColor: getComputedStyle(document.querySelector(".media-print-probe")!).color };
    });
    expect(candidateMedia.printColor).toBe(candidateMedia.defaultColor);
    const candidateDataUrl = await candidate.getAttribute("src");
    expect(candidateDataUrl).toContain("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+");
    const candidateBytesUrl = candidateDataUrl?.split("#", 1)[0];
    const appliedBytesUrl = appliedDataUrl?.split("#", 1)[0];
    if (useAlternateImage) expect(candidateBytesUrl).toBe(appliedBytesUrl);
    else expect(candidateBytesUrl).not.toBe(appliedBytesUrl);
    await candidate.scrollIntoViewIfNeeded();
    await page.waitForTimeout(100);
    await page.screenshot({ path: "/tmp/konae-u1b2-assets-luna/proposal-public-image.png" });
    const candidateFrame = page.frames().find((frame) => frame.parentFrame() !== null);
    expect(await candidateFrame?.evaluate(() => [...document.images].map((image) => image.src))).toContain(await candidate.getAttribute("src"));
    expect(await objectUrlsIn()).toBe(0);
    await showSection(page, "Describe");
    await page.getByRole("button", { name: "Discard", exact: true }).click();
    await showSection(page, "Preview");
    await expect.poll(() => preview.locator("#asset-image").evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(4);
    await expect(preview.locator("#asset-image")).toHaveAttribute("src", appliedDataUrl!);
    await expect.poll(() => preview.locator("#stylesheet-probe").evaluate((node) => getComputedStyle(node).color)).toBe("rgb(16, 32, 48)");
    await expect(preview.getByText("Saved row", { exact: true })).toBeVisible();
    await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
    await expect(preview.getByRole("img", { name: "proposal-proof.png" })).toBeVisible();
    expect(await objectUrlsIn()).toBe(0);
    expect(await page.evaluate(() => JSON.stringify(Object.entries(localStorage).sort(([a], [b]) => a.localeCompare(b))))).toBe(storageBefore);
  }

  fixture.setScenario("asset-apply");
  await showSection(page, "Describe");
  await askFixture(page, "Show the candidate public image and apply it.");
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 30_000 });
  const candidate = page.frameLocator('iframe[title="Mini-app preview"]').locator("#asset-image");
  await expect.poll(() => candidate.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(4);
  await expect.poll(() => page.frameLocator('iframe[title="Mini-app preview"]').locator("#stylesheet-probe").evaluate((node) => getComputedStyle(node).color)).toBe("rgb(16, 32, 48)");
  expect(await candidate.getAttribute("src")).not.toBe(appliedDataUrl);
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await showSection(page, "Preview");
  await expect.poll(() => preview.locator("#asset-image").evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(4);
  expect(await preview.locator("#asset-image").getAttribute("src")).toBe(await candidate.getAttribute("src"));
  await expect(preview.getByText("Saved row", { exact: true })).toBeVisible();
  await expect(preview.getByText("Alice", { exact: true })).toBeVisible();
  await expect(preview.locator("#proposal-files")).toHaveText("proposal-proof.png");
  expect(await objectUrlsIn()).toBe(0);
  await waitForKernelSave(page);
  const savedSources = await readSavedSources(page, projectId);
  expect(savedSources.files["public/retained.png"]).toBe(retainedAsset);
  expect(savedSources.files["public/applied.png"]).toBeDefined();
  expect(savedSources.files["public/candidate.png"]).toBeDefined();
  expect(savedSources.encodings?.["public/applied.png"]).toBe("base64");
  expect(savedSources.encodings?.["public/candidate.png"]).toBe("base64");
});

test("manual action edits leave the running preview unchanged until Run", async ({ page }) => {
  await createProject(page, "Run applies action edits", "tasks");
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Preview");
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await preview.locator("#task-input").fill("Before edit");
  await preview.getByRole("button", { name: "Add task" }).click();
  await expect(preview.getByText("Before edit", { exact: true })).toBeVisible();

  const actions = await openFile(page, "actions.json");
  const original = await actions.inputValue();
  expect(original).toContain('"tasks.list"');
  await actions.fill(original.replace('"tasks.list": {', '"tasks.pending": {'));
  await showSection(page, "Preview");
  await expect(preview.getByText("Before edit", { exact: true })).toBeVisible();
  await preview.locator("#task-input").fill("Still using the checked preview");
  await preview.getByRole("button", { name: "Add task" }).click();
  await expect(preview.getByText("Still using the checked preview", { exact: true })).toBeVisible();

  await showSection(page, "Files");
  await page.getByRole("button", { name: "Check", exact: true }).click();
  await expect(page.locator(".preview-problems")).toContainText("Server action not declared.", { timeout: 30_000 });
});

test("a stale contact email save shows the current value", async ({ page, context, browser }) => {
  const email = `owner-${crypto.randomUUID()}@example.test`;
  const password = "Owner-test-password-2026";
  const client = `192.0.2.${1 + Math.floor(Math.random() * 254)}`;
  const signup = await page.request.post("/_konae/owner/signup", { headers: { "x-forwarded-for": client }, data: { email, password, setupKey: "local-development-key" } });
  expect(signup.ok()).toBe(true);
  const projectPath = await createProject(page, `Email conflict ${Date.now()}`);
  await publish(page);
  await fromMore(page, "App settings");
  let settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await settings.getByRole("tab", { name: "Contact email" }).click();
  await expect(settings.getByLabel("Email", { exact: true })).toHaveValue(email);

  const secondContext = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const second = await secondContext.newPage();
    await second.goto(projectPath);
    const signIn = second.locator("form#auth");
    await signIn.getByLabel("Email").fill(email);
    await signIn.getByLabel("Password").fill(password);
    await second.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(second).toHaveURL(new RegExp(`${projectPath}$`));
    await fromMore(second, "App settings");
    const otherSettings = second.getByRole("dialog", { name: "App settings", exact: true });
    await otherSettings.getByRole("tab", { name: "Contact email" }).click();
    await otherSettings.getByLabel("Email", { exact: true }).fill("second@example.test");
    await otherSettings.getByRole("button", { name: "Change email", exact: true }).click();
    await expect(otherSettings.getByRole("status")).toContainText("Saved.");
  } finally {
    await secondContext.close();
  }

  await settings.getByLabel("Email", { exact: true }).fill("first@example.test");
  await settings.getByRole("button", { name: "Change email", exact: true }).click();
  await expect(settings.getByRole("alert")).toContainText("The setting changed since you read it.");
  await expect(settings.getByLabel("Email", { exact: true })).toHaveValue("second@example.test");
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

test("Modules settings show fixed modules and read back seo after reload", async ({ page }) => {
  const projectPath = await createProject(page, "Module settings");
  const projectId = projectPath.split("/").at(-1)!;
  await seedRetainedBinary(page, projectId);
  await page.reload();
  await fromMore(page, "App settings");
  let settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await settings.getByRole("tab", { name: "Modules", exact: true }).click();
  await expect(settings.getByText("This project is not published yet. Module settings travel with the first release.")).toBeVisible();
  await expect(settings.locator(".module-settings-card")).toHaveCount(1);
  await expect(settings.locator(".module-settings-card").first()).toContainText("Search listing");
  await expect(settings.locator(".module-settings-card").first()).toContainText("seo");
  await expect(settings.locator(".module-settings-fixed-item")).toHaveCount(5);
  await settings.getByRole("button", { name: "Close App settings", exact: true }).click();
  await publish(page);
  await fromMore(page, "App settings");
  settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await settings.getByRole("tab", { name: "Modules", exact: true }).click();
  await expect(settings.getByLabel("Canonical address", { exact: true }).locator("option")).toContainText(["Custom domain", "Generated hostname", "Path address"]);
  await expect(settings.getByLabel("Card image", { exact: true })).toHaveValue("");
  await expect(settings.getByLabel("Card image", { exact: true }).locator("option")).toContainText(["public/retained.png"]);
  await settings.getByLabel("Card image", { exact: true }).selectOption("public/retained.png");
  await settings.getByRole("button", { name: "Save changes", exact: true }).click();
  await expect(settings.getByText("seo settings saved.", { exact: true })).toBeVisible();
  await expect(settings.getByLabel("Card image", { exact: true })).toHaveValue("public/retained.png");
  await expect(settings.getByRole("switch", { name: "Turn Search listing on", exact: true })).toHaveAttribute("aria-checked", "false");
  await settings.getByRole("switch", { name: "Turn Search listing on", exact: true }).click();
  await expect(settings.locator(".module-settings-card").filter({ hasText: "seo" }).getByText("On", { exact: true })).toBeVisible();
  await expect(settings.getByRole("link", { name: "Open sitemap", exact: true })).toBeVisible();
  await page.reload();
  await fromMore(page, "App settings");
  settings = page.getByRole("dialog", { name: "App settings", exact: true });
  await settings.getByRole("tab", { name: "Modules", exact: true }).click();
  await expect(settings.locator(".module-settings-card").filter({ hasText: "seo" }).getByText("On", { exact: true })).toBeVisible();
});

test("the build agent confirms module changes and reads them back", async ({ page }) => {
  const projectPath = await createProject(page, "Agent module settings");
  await publish(page);
  await installModuleSetFixture(page);
  await connectProposalFixture(page);
  const calls: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname !== "/mcp") return;
    try {
      const body = request.postDataJSON();
      if (body.method === "tools/call") calls.push(body.params.name);
    } catch { }
  });
  await askFixture(page, "Turn on seo and use the path address.");
  const confirm = page.getByRole("dialog", { name: "Allow the agent to do this?" });
  await expect(confirm.getByText("The agent will turn on Search listing (seo) and set Canonical address to Path address.", { exact: true })).toBeVisible({ timeout: 30_000 });
  await confirm.getByRole("button", { name: "Allow", exact: true }).click();
  await expect(page.getByText("I read the module state after the change.", { exact: true })).toBeVisible({ timeout: 30_000 });
  expect(calls).toContain("module_set");
  expect(calls.indexOf("modules_read")).toBeGreaterThan(calls.indexOf("module_set"));
  const cookie = (await page.context().cookies()).map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", page.url()).href, { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
  const readback = await client.call<{ modules: { seo: { state: { on: boolean; canonical: string } } } }>("modules_read", { projectId: projectPath.split("/").at(-1) });
  expect(readback.modules.seo.state).toMatchObject({ on: true, canonical: "path" });

  const secondPath = await createProject(page, "Declined module change");
  await publish(page);
  await installModuleSetFixture(page);
  await askFixture(page, "Turn on seo and use the path address.");
  const declined = page.getByRole("dialog", { name: "Allow the agent to do this?" });
  await expect(declined).toBeVisible({ timeout: 30_000 });
  await declined.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByText("I read the module state after the change.", { exact: true }).last()).toBeVisible({ timeout: 30_000 });
  const current = await client.call<{ modules: { seo: { state: { on: boolean } } } }>("modules_read", { projectId: secondPath.split("/").at(-1) });
  expect(current.modules.seo.state.on).toBe(false);
});

test("browser builds, republishes, backs up and exports a persistent app", async ({ page, context }, testInfo) => {
  await createProject(page, `Browser tasks ${Date.now()}`);
  const explorer = await openExplorer(page);
  await page.getByRole("button", { name: "File list", exact: true }).click();
  await expect(explorer).toBeHidden();
  await page.keyboard.press("Control+b");
  await expect(explorer).toBeVisible();
  await explorer.getByRole("button", { name: "New file" }).click();
  await page.getByLabel("File path").fill("src/label.js");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.getByLabel("src/label.js code editor").fill("export const label = 'Compiled in the browser';");
  await expect(explorer).toBeVisible();
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
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  await waitForKernelSave(page);
  await page.reload();
  await expect(preview.getByText("Compiled in the browser")).toBeVisible();
  const url = await publish(page);
  const app = await context.newPage();
  await app.goto(url);
  await expect(app.getByText("Compiled in the browser")).toBeVisible();
  await app.getByPlaceholder(/Add one small thing/).fill("Survives a new release");
  await app.getByRole("button", { name: "Add task" }).click();
  await expect(app.getByText("Survives a new release")).toBeVisible();
  await app.goto(new URL("/_konae/login", url!).href);
  await signInApp(app);
  await expect(app.getByText("Survives a new release")).toBeVisible();
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

test("the editor reloads sources published from an app folder", async ({ page, baseURL }, testInfo) => {
  const projectId = (await createProject(page, `Folder publish ${Date.now()}`)).split("/").pop()!;
  await publish(page);
  const token = await page.evaluate(async (projectId) => {
    const rpc = async (method: string, params: object) => (await (await fetch("/mcp", {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25" },
      body: JSON.stringify({ jsonrpc: "2.0", id: crypto.randomUUID(), method, params }),
    })).json()).result;
    await rpc("initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "e2e", version: "0" } });
    return (await rpc("tools/call", { name: "token_create", arguments: { scopes: ["read", "write", "publish"], projectIds: [projectId], ttlSeconds: 600 } })).structuredContent.result.token as string;
  }, projectId);
  const folder = new McpClient(new URL("/mcp", baseURL).href, { token });
  const saved = await folder.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const project = JSON.parse(new TextDecoder().decode(await folder.download(saved.transfer)));
  project.files["actions.json"] = project.files["actions.json"].replace("LIMIT 100", "LIMIT 50");
  project.files["app.js"] = "window.folderRelease = true;";
  const artifact = { version: 1, entrypoint: "index.html", files: { "index.html": "<p>Published from a folder</p>" } };
  await publishProject(folder, project, () => ({ project, visibility: "public", artifact }), async () => true, undefined, undefined, saved.revision);

  const dialog = page.getByRole("dialog", { name: /^Deploy / });
  const app = await openFile(page, "app.js");
  await expect(app).toHaveValue("window.folderRelease = true;", { timeout: 15_000 });
  await page.getByRole("banner").getByRole("button", { name: "Deploy changes", exact: true }).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(/The editor holds other files/)).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("editor-after-folder-publish.png"), fullPage: true });
});

test("a direct reload compiles the selected TypeScript project before its first preview", async ({ page }, testInfo) => {
  const projectPath = await createProject(page, `Direct TypeScript load ${Date.now()}`, "tasks");
  await writeTypedPreview(page, "Compiled on direct reload");

  await page.goto(projectPath);
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByRole("heading", { name: "Compiled on direct reload" })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("direct-typescript-reload.png"), fullPage: true });
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Projects" }).click();
  const projectCard = page.locator(`a.project-card-link[href="${projectPath}"]`);
  await expect(projectCard).toBeVisible();
  await projectCard.click();
  await expect(preview.getByRole("heading", { name: "Compiled on direct reload" })).toBeVisible();
});

test("switching between cached TypeScript projects compiles each selected source", async ({ page }) => {
  const firstPath = await createProject(page, `Typed first ${Date.now()}`, "tasks");
  await writeTypedPreview(page, "First cached app");
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Projects" }).click();
  await page.getByRole("button", { name: "New project" }).click();
  await page.getByRole("textbox", { name: "Project name", exact: true }).fill(`Typed second ${Date.now()}`);
  await page.getByText("Start from a template instead", { exact: true }).click();
  await page.getByRole("radio", { name: /^Shared list/ }).check();
  await page.getByRole("button", { name: /^Create/ }).click();
  const secondPath = new URL(page.url()).pathname;
  await writeTypedPreview(page, "Second cached app");

  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Projects" }).click();
  await page.locator(`a.project-card-link[href="${secondPath}"]`).click();
  await expect(preview.getByRole("heading", { name: "Second cached app" })).toBeVisible();
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Projects" }).click();
  await page.locator(`a.project-card-link[href="${firstPath}"]`).click();
  await expect(preview.getByRole("heading", { name: "First cached app" })).toBeVisible();
  await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Projects" }).click();
  await page.locator(`a.project-card-link[href="${secondPath}"]`).click();
  await expect(preview.getByRole("heading", { name: "Second cached app" })).toBeVisible();
});

test("Try again compiles sources after a project load fails", async ({ page }) => {
  const projectPath = await createProject(page, `Retry TypeScript ${Date.now()}`, "tasks");
  const projectId = projectPath.slice("/app/".length);
  await writeTypedPreview(page, "Retry succeeded");
  let refuseReadsUntilError = true;
  const refusedRequestIds: string[] = [];
  await page.route("**/mcp", async (route) => {
    const body = route.request().postData() ?? "";
    const sourceRead = sourceReadRequest(body);
    if (refuseReadsUntilError && isFullSourceRead(body) && sourceRead?.projectId === projectId) {
      refusedRequestIds.push(sourceRead.requestId!);
      await route.abort();
      return;
    }
    await route.continue();
  });
  await page.reload();
  await expect(page.getByRole("heading", { name: "Could not open this project" })).toBeVisible();
  refuseReadsUntilError = false;
  expect(refusedRequestIds.length).toBeGreaterThan(0);
  await page.unroute("**/mcp");
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "Retry succeeded" })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
});

test("a delayed Try again response cannot select a project after leaving it", async ({ page }) => {
  const projectPath = await createProject(page, `Abandoned retry ${Date.now()}`, "tasks");
  const projectId = projectPath.slice("/app/".length);
  await writeTypedPreview(page, "Retry after leaving");
  await page.addInitScript(() => {
    const state = window as typeof window & { __holdNextSourceRead?: boolean; __sourceReadHeld?: Promise<void>; __releaseSourceRead?: () => void };
    const isFullSourceRead = (body: string) => {
      try {
        const request = JSON.parse(body);
        const args = request.params?.name === "project_sources_read" ? request.params.arguments : undefined;
        return Boolean(args?.projectId && args.requestId && !args.cancelRequestId && args.metadataOnly !== true);
      } catch { return false; }
    };
    let signalHeld!: () => void;
    state.__sourceReadHeld = new Promise<void>((resolve) => { signalHeld = resolve; });
    let releaseHeld!: () => void;
    const original = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const body = typeof init?.body === "string" ? init.body : "";
      const response = await original(input, init);
      if (state.__holdNextSourceRead && isFullSourceRead(body)) {
        state.__holdNextSourceRead = false;
        signalHeld();
        await new Promise<void>((resolve) => { releaseHeld = resolve; });
      }
      return response;
    };
    state.__releaseSourceRead = () => releaseHeld?.();
  });
  let refuseReadsUntilError = true;
  const refusedRequestIds: string[] = [];
  await page.route("**/mcp", async (route) => {
    const body = route.request().postData() ?? "";
    const sourceRead = sourceReadRequest(body);
    if (refuseReadsUntilError && isFullSourceRead(body) && sourceRead?.projectId === projectId) {
      refusedRequestIds.push(sourceRead.requestId!);
      await route.abort();
      return;
    }
    await route.continue();
  });
  await page.reload();
  await expect(page.getByRole("heading", { name: "Could not open this project" })).toBeVisible();
  refuseReadsUntilError = false;
  expect(refusedRequestIds.length).toBeGreaterThan(0);
  await page.unroute("**/mcp");
  await page.evaluate(() => { (window as typeof window & { __holdNextSourceRead?: boolean }).__holdNextSourceRead = true; });
  await page.getByRole("button", { name: "Try again" }).click();
  await page.evaluate(async () => { await (window as typeof window & { __sourceReadHeld?: Promise<void> }).__sourceReadHeld; });
  await page.goBack();
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();
  await page.evaluate(() => (window as typeof window & { __releaseSourceRead?: () => void }).__releaseSourceRead?.());
  await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();
  await page.locator(`a.project-card-link[href="${projectPath}"]`).click();
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "Retry after leaving" })).toBeVisible();
});

test("full document navigation cancels allocated source reads before their responses arrive", async ({ page }) => {
  const projectPath = await createProject(page, `Reload transfer probe ${Date.now()}`, "tasks");
  const allocatedTransfers: string[] = [];
  let completedCancels = 0;
  let holdNext = false;
  let signalReady: () => void = () => {};
  let releaseResponse: () => void = () => {};
  await page.route("**/mcp", async (route) => {
    const body = route.request().postData() ?? "";
    if (body.includes('"cancelRequestId"')) {
      const response = await route.fetch();
      completedCancels++;
      await route.fulfill({ response }).catch(() => {});
      return;
    }
    if (holdNext && body.includes('"project_sources_read"') && !body.includes('"metadataOnly":true')) {
      holdNext = false;
      const response = await route.fetch();
      const responseText = await response.text();
      const result = JSON.parse(responseText).result.structuredContent.result;
      expect(result.transfer.transferId).toBeTruthy();
      allocatedTransfers.push(result.transfer.transferId);
      signalReady();
      await new Promise<void>((resolve) => { releaseResponse = resolve; });
      await route.fulfill({ response }).catch(() => {});
      return;
    }
    await route.continue();
  });
  for (let attempt = 0; attempt < 9; attempt += 1) {
    const ready = new Promise<void>((resolve) => { signalReady = resolve; });
    holdNext = true;
    await page.goto("/");
    await page.locator(`a.project-card-link[href="${projectPath}"]`).click();
    await ready;
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Projects" })).toBeVisible();
    await expect.poll(() => completedCancels).toBe(attempt + 1);
    releaseResponse();
  }
  await page.unroute("**/mcp");
  expect(allocatedTransfers).toHaveLength(9);
  expect(completedCancels).toBeGreaterThanOrEqual(9);
  await page.goto(projectPath);
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "Tiny Tasks" })).toBeVisible();
});

test("the editor reloads clean sources and keeps a dirty draft when another tab saves", async ({ page }, testInfo) => {
  const projectId = (await createProject(page, `Source conflict ${Date.now()}`, "tasks")).split("/").pop()!;
  await publish(page);
  const token = await page.evaluate(async (projectId) => {
    const rpc = async (method: string, params: object) => (await (await fetch("/mcp", {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25" },
      body: JSON.stringify({ jsonrpc: "2.0", id: crypto.randomUUID(), method, params }),
    })).json()).result;
    await rpc("initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "e2e", version: "0" } });
    return (await rpc("tools/call", { name: "token_create", arguments: { scopes: ["read", "write"], projectIds: [projectId], ttlSeconds: 600 } })).structuredContent.result.token as string;
  }, projectId);
  const outside = new McpClient(new URL("/mcp", page.url()).href, { token });
  const saved = await outside.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const project = JSON.parse(new TextDecoder().decode(await outside.download(saved.transfer)));
  const app = await openFile(page, "app.js");
  await expect(app).toBeVisible();

  const saveOutside = async (filesToChange: Record<string, string>, revision: number) => {
    const files = { ...project.files, ...filesToChange };
    const upload = await outside.upload(new TextEncoder().encode(JSON.stringify({ ...project, files, updatedAt: Date.now() })), projectId);
    try { return await outside.call<{ revision: number }>("project_sources_save", { projectId, transferId: upload.transferId, expectedRevision: revision }); }
    finally { await outside.cancel(upload.transferId); }
  };

  const remoteFiles = (value: string) => ({
    "app.js": 'import { remoteLabel } from "./remote.ts"; document.body.dataset.remoteLabel = remoteLabel;',
    "remote.ts": `export const remoteLabel: string = "${value}";`,
  });
  const revision = (await saveOutside(remoteFiles("clean TypeScript reload"), saved.revision)).revision;
  await expect(app).toHaveValue(remoteFiles("clean TypeScript reload")["app.js"], { timeout: 15_000 });
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').locator("body")).toHaveAttribute("data-remote-label", "clean TypeScript reload", { timeout: 15_000 });
  const holdNextInspection = () => {
    let releaseInspection!: () => void;
    let inspectionPending!: () => void;
    const release = new Promise<void>((resolve) => { releaseInspection = resolve; });
    const held = new Promise<void>((resolve) => { inspectionPending = resolve; });
    let waitForRead = true;
    let holdInspection = true;
    const route = page.route("**/mcp", async (requestRoute) => {
      const body = requestRoute.request().postData() ?? "";
      if (body.includes('"project_sources_read"')) {
        waitForRead = false;
        await requestRoute.continue();
        return;
      }
      if (!waitForRead && holdInspection && body.includes('"project_inspect"')) {
        holdInspection = false;
        const response = await requestRoute.fetch();
        inspectionPending();
        await release;
        await requestRoute.fulfill({ response });
        return;
      }
      await requestRoute.continue();
    });
    return { held, release: releaseInspection, route };
  };
  let automatic = holdNextInspection();
  const newer = saveOutside(remoteFiles("newer TypeScript source"), revision);
  await newer;
  await automatic.held;
  await app.fill("window.localDraft = 'keep this';");
  automatic.release();
  await expect(app).toHaveValue("window.localDraft = 'keep this';");
  await expect(page.getByText("This app changed on another machine. Your draft is still here.")).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("dirty-source-conflict.png"), fullPage: true });

  await page.unroute("**/mcp");
  const explicit = holdNextInspection();
  await page.getByRole("button", { name: "Reload saved sources" }).click();
  await explicit.held;
  await app.fill("window.explicitDraft = 'keep this too';");
  explicit.release();
  await expect(app).toHaveValue("window.explicitDraft = 'keep this too';");
  await expect(page.getByText("This app changed on another machine. Your draft is still here.")).toBeVisible();
  await page.unroute("**/mcp");
  await page.getByRole("button", { name: "Reload saved sources" }).click();
  await expect(app).toHaveValue(remoteFiles("newer TypeScript source")["app.js"]);
});

test("the editor keeps edits made while a release response is pending", async ({ page }) => {
  await createProject(page, `Publish draft ${Date.now()}`, "tasks");
  await publish(page);
  const editor = await openFile(page, "app.js");
  await editor.fill("window.releaseSource = true;");
  await waitForKernelSave(page);

  let releaseResponseReady!: () => void;
  let releaseResponse!: () => void;
  const responseReady = new Promise<void>((resolve) => { releaseResponseReady = resolve; });
  const held = new Promise<void>((resolve) => { releaseResponse = resolve; });
  let holdApply = true;
  let sourceSavesWhileHeld = 0;
  page.on("request", (request) => {
    const body = request.postData() ?? "";
    if (body.includes('"project_sources_save"')) sourceSavesWhileHeld++;
  });
  await page.route("**/mcp", async (route) => {
    const body = route.request().postData() ?? "";
    if (holdApply && body.includes('"release_apply"')) {
      holdApply = false;
      releaseResponseReady();
      await held;
      await route.continue();
      return;
    }
    await route.continue();
  });

  await page.getByRole("banner").getByRole("button", { name: "Deploy changes", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Deploy changes", exact: true }).click();
  await responseReady;
  await editor.fill("window.draftDuringPublish = true;", { force: true });
  await page.waitForTimeout(800);
  expect(sourceSavesWhileHeld).toBe(0);
  releaseResponse();
  await expect(editor).toHaveValue("window.draftDuringPublish = true;");
  await expect(page.locator("a.published-url")).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "Done", exact: true }).click();
  await page.unroute("**/mcp");
  await waitForKernelSave(page);
});

test("public notes authenticate users and isolate private rows", async ({ page, context }, testInfo) => {
  await createProject(page, `Personal notes ${Date.now()}`, "notes");
  const preview = page.frameLocator('iframe[title="Mini-app preview"]');
  const composer = preview.getByPlaceholder("Keep a thought…");
  await expect(async () => {
    if (!(await composer.isVisible())) await preview.getByRole("button", { name: "Sign in", exact: true }).click({ timeout: 2_000 });
    await expect(composer).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 20_000 });
  await preview.getByPlaceholder("Keep a thought…").fill("Alice preview note");
  await preview.getByRole("button", { name: "Save", exact: true }).click();
  await expect(preview.getByText("Alice preview note")).toBeVisible();
  await showSection(page, "Access");
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
  const signup = page.getByLabel("Who can create an app account");
  await expect(signup).toHaveValue("invitation");
  await expect(page.getByText("Only people you invite by email can create an app account", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Deploy private", exact: true }).click();
  const link = page.locator("a.published-url");
  await expect(link).toBeVisible({ timeout: 30_000 });
  const url = (await link.getAttribute("href"))!;
  await expect(page.getByText("New app accounts: Invitation only.")).toBeVisible();
  await page.getByRole("button", { name: "Invite people", exact: true }).click();

  const app = await context.newPage();
  await app.goto(url);
  await expect(app.getByRole("heading", { name: "Sign in to this app" })).toBeVisible();
  await expect(app.getByRole("button", { name: "Create account" })).toHaveCount(0);

  const users = page.getByRole("region", { name: "App users" });
  await expect(users.getByLabel("Who can create an app account")).toHaveValue("invitation");
  const invitee = `guest-${crypto.randomUUID()}@example.test`;
  await users.getByLabel("Email addresses").fill(invitee);
  await users.getByRole("button", { name: "Send invitation", exact: true }).click();
  await expect(users.getByRole("list", { name: "Invitation results" })).toContainText("Send this link to the person yourself");
  const invitations = users.getByRole("group", { name: "Invitations" });
  const row = invitations.getByRole("row").filter({ hasText: invitee });
  await expect(row).toContainText("Pending");
  await expect(row).toContainText("Handed to mail");
  await expect(row).toContainText(/Expires /);
  await expect(users.getByRole("group", { name: "App accounts" }).getByRole("row").filter({ hasText: invitee })).toContainText("Pending");

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

  await page.setViewportSize({ width: 1280, height: 2000 });
  await page.evaluate(() => window.scrollTo(0, 0));
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: testInfo.outputPath(`users-${scheme}.png`), animations: "disabled" });
  }

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
  await page.getByText("What each action does", { exact: true }).click();
  await expect(page.getByText("Receive. Only its route, POST /hooks/orders, runs it, for a signed request or an API key.")).toBeVisible();
  await expect(page.getByText('Mark a row done. Anyone can run it. When no row changes it refuses with "That task is gone.".')).toBeVisible();
  await waitForKernelSave(page);

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
  const anonymous = await browser.newContext();
  const visitor = await anonymous.newPage();
  await visitor.goto(url);
  await addTask(visitor, "From a visitor");
  await anonymous.close();
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
