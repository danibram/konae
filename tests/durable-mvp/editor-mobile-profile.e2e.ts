import { seedHistory, inertLabel, oldestLabel, originalTail, modelTail } from "./seed-history.mjs";
import { devices, webkit } from "playwright";
import { DatabaseSync } from "node:sqlite";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { expect, test, type Page } from "playwright/test";
import { McpClient, type Transfer } from "../../src/mcp/client.ts";
import { publishProject } from "../../src/deployments/release.ts";

const runCommand = promisify(execFile);
async function signupOwner(page: Page, client: string, email: string, password = "Owner-test-password-2026") {
  const response = await page.request.post("/_konae/owner/signup", {
    headers: { "x-forwarded-for": client },
    data: { email, password, setupKey: "local-development-key" },
  });
  return { ok: response.ok(), status: response.status() };
}

async function logoutOwner(page: Page) {
  await page.request.post("/_konae/owner/logout");
  await page.context().clearCookies();
}

test.beforeEach(async ({ page }) => {
  const client = `198.51.100.${1 + Math.floor(Math.random() * 254)}`;
  const response = await signupOwner(page, client, `owner-${crypto.randomUUID()}@example.test`);
  expect(response.ok).toBe(true);
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
  const signup = await signupOwner(page, client, email, password);
  expect(signup.ok).toBe(true);
  const projectPath = await createProject(page, `Return here ${Date.now()}`);
  await logoutOwner(page);

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
const actualReadTail = "ORIGINAL_TOOL_ONLY_TAIL_LOCAL_FIXTURE";
const longScratchFile = "/*\n" + "A synthetic retained tool detail line with no service credential.\n".repeat(520) + actualReadTail + "\n*/\ndocument.documentElement.dataset.proposal = 'ready';\n";

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
  const saved = await client.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const document = JSON.parse(new TextDecoder().decode(await client.download(saved.transfer))) as { files: Record<string, string>; encodings?: Record<string, "base64"> };
  return { ...document, revision: saved.revision };
}

async function saveFixtureSources(page: Page, projectId: string, revision: number, source: { files: Record<string, string>; encodings?: Record<string, "base64"> }) {
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const client = new McpClient(new URL("/mcp", page.url()).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(source)), projectId);
  await client.call("project_sources_save", { projectId, transferId: transfer.transferId, expectedRevision: revision });
}

async function waitForKernelSave(page: Page) {
  await page.waitForTimeout(1_000);
  await expect(page.getByRole("banner").getByText("Saving")).toHaveCount(0, { timeout: 30_000 });
  await expect(page.getByRole("banner").getByText("Not saved")).toHaveCount(0);
}

async function showSection(page: Page, name: "Preview" | "Describe" | "Access" | "Files" | "Data" | "Uploads") {
  const navigation = page.getByRole("navigation", { name: "Workspace sections" });
  await expect(navigation).toBeVisible({timeout: 30000});
  const tab = navigation.getByRole("tab", { name, exact: true });
  if (await tab.isVisible()) {
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
  let scenario: "discard" | "apply" | "asset-discard" | "asset-discard-applied" | "asset-apply" | "blocked" | "drift" | "schema" | "invite" | "context" | "build-fail" | "preview-behind" | "long-read" = "discard";
  let step = 0;
  let inviteEmail = "guest@example.test";
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
    if (scenario === "invite") {
      if (step === 0) tool = { name: "app_invitations_create", input: { emails: [inviteEmail], idempotencyKey: `fixture-invite-${inviteEmail}` } };
      else if (step === 1) tool = { name: "app_invitations_list", input: {} };
    } else if (scenario === "long-read") {
      if (step === 0) tool = { name: "write", input: { path: "/workspace/app.js", content: longScratchFile } };
      if (step === 1) tool = { name: "read", input: { path: "/workspace/app.js" } };
      if (step === 2) tool = { name: "write", input: { path: "/workspace/index.html", content: html } };
      if (step === 3) tool = { name: "validate_capsule", input: {} };
    } else if (scenario === "blocked") {
      if (step === 0) tool = { name: "write", input: { path: "/workspace/actions.json", content: invalidActions } };
      else if (step === 1) tool = { name: "validate_capsule", input: {} };
    } else if (scenario === "context") {
      // A plain answer lets the test inspect the actual restored provider wire without another mutation.
    } else if (step === 0) {
      const path = scenario === "preview-behind" || scenario === "schema" ? "/workspace/schema.sql" : "/workspace/index.html";
      const content = scenario === "preview-behind" ? `
CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  done INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  due_at INTEGER
);
      ` : scenario === "schema" ? `CREATE TABLE IF NOT EXISTS notes (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  owner_id TEXT NOT NULL,\n  title TEXT NOT NULL,\n  created_at INTEGER NOT NULL\n);\nCREATE INDEX IF NOT EXISTS notes_owner_created_at ON notes(owner_id, created_at);\n` : scenario === "asset-discard" || scenario === "asset-discard-applied" || scenario === "asset-apply" ? assetHtml(scenario === "asset-discard-applied") : html;
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
  return { setScenario(value: typeof scenario) { scenario = value; }, setInviteEmail(value: string) { inviteEmail = value; } };
}

async function installModuleSetFixture(page: Page, input: object = { module: "seo", on: true, options: { canonical: "path" }, expected: { on: false, canonical: null, image: null } }) {
  let step = 0;
  await page.route("https://api.anthropic.com/**", async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-methods": "POST,OPTIONS", "access-control-allow-headers": "*" } });
      return;
    }
    const response = step++ === 0
      ? providerReply({ name: "module_set", input })
      : providerReply(undefined, "I read the module state after the change.");
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-headers": "*", "access-control-expose-headers": "*", "cache-control": "no-cache" },
      body: response,
    });
  });
}

async function installActionsUpgradeFixture(page: Page) {
  let step = 0;
  const actions = JSON.stringify({ version: 1, actions: {
    "hello.read": { kind: "query", access: "anonymous", statement: "SELECT 1 AS value LIMIT 1", bindings: [] },
  } });
  await page.route("https://api.anthropic.com/**", async (route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: { "access-control-allow-origin": route.request().headers().origin ?? "*", "access-control-allow-methods": "POST,OPTIONS", "access-control-allow-headers": "*" } });
      return;
    }
    const tools = [
      { name: "write", input: { path: "/workspace/actions.json", content: actions } },
      { name: "validate_capsule", input: {} },
      { name: "module_set", input: { module: "actions", on: true, expected: { on: false } } },
      { name: "write", input: { path: "/workspace/actions.json", content: actions } },
      { name: "validate_capsule", input: {} },
    ];
    const tool = tools[step++];
    const origin = route.request().headers().origin ?? "*";
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      headers: { "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-expose-headers": "*", "cache-control": "no-cache" },
      body: providerReply(tool, "I read the module state after the change."),
    });
  });
}

async function connectProposalFixture(page: Page, fixtureKey = "fixture-key-unused") {
  await showSection(page, "Describe");
  await page.getByRole("button", { name: /Connect a model/ }).click();
  const dialog = page.getByRole("dialog", { name: "Connect a model" });
  await dialog.getByLabel("Anthropic API key").fill(fixtureKey);
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
  if (item === "App settings") await expect(page.getByRole("dialog", { name: "App settings", exact: true })).toBeVisible();
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


async function proveLocalEditor(page: Page, origin: string) {
  const backendCalls: Array<{ method: string; path: string }> = [];
  const providers: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.origin === origin && (url.pathname === "/mcp" || url.pathname.startsWith("/_konae/"))) {
      backendCalls.push({ method: request.method(), path: url.pathname });
    }
    if (url.hostname === "api.anthropic.com" && request.method() === "POST") providers.push(url.pathname);
  });
  const fixture = await installProposalFixture(page);
  fixture.setScenario("drift");
  const route = process.env.KONAE_GATE31_LOCAL_PATH ?? "/local";
  await page.goto(origin + route);
  await expect(page.getByText(/local.only|this browser only/i).first()).toBeVisible();
  await page.getByRole("button",{name:"Model key",exact:true}).click();
  const key = page.getByRole("dialog",{name:"Connect a model"});
  await key.getByLabel("Anthropic API key").fill("local-only-fixture-key");
  await key.getByRole("button",{name:"Save",exact:true}).click();
  await expect(key).toBeHidden();
  await askFixture(page, "Make the first local browser-only fixture preview.");
  const preview = page.frameLocator('iframe[title="Local-only app preview"]');
  await expect(preview.getByRole("heading", { name: "PROPOSAL PREVIEW", exact: true })).toBeVisible({ timeout: 40000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(preview.getByRole("heading", { name: "PROPOSAL PREVIEW", exact: true })).toBeVisible();
  expect(providers.length).toBeGreaterThan(0);
  expect(backendCalls).toEqual([]);
}

test("explicit local editor uses no owner APIs even when an owner cookie is present", async ({ page, baseURL }) => {
  const session = await page.request.get("/_konae/owner/session");
  expect((await session.json()).signedIn).toBe(true);
  await proveLocalEditor(page, new URL(baseURL!).origin);
});

test("fresh mobile WebKit has a usable local-only agent, Gate, Preview and Apply", async ({ baseURL }) => {
  const browser = await webkit.launch({ channel: undefined, args: [], timeout: 15_000 });
  const context = await browser.newContext({ ...devices["iPhone 13"], baseURL });
  try {
    expect(await context.cookies()).toHaveLength(0);
    const page = await context.newPage();
    await proveLocalEditor(page, new URL(baseURL!).origin);
  } finally {
    await context.close();
    await browser.close();
  }
});

test("mobile WebKit opens a bounded 1,000-entry owner history without a key", async ({ page, baseURL }) => {
  const projectPath = await createProject(page, "Bounded mobile owner history", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const origin = new URL(baseURL!).origin;
  const owner = await (await page.request.get("/_konae/owner/session")).json();
  const cookie = (await page.context().cookies()).map(({name,value})=>name+"="+value).join("; ");
  const fixture = await seedHistory({projectId,fetcher:(input,init)=>{
    const headers=new Headers(init?.headers); headers.set("cookie",cookie); headers.set("origin",origin);
    return fetch(new URL(input,origin),{...init,headers});
  }});
  const webkitBrowser = await webkit.launch({ channel: undefined, args: [], timeout: 15_000 });
  const mobileContext = await webkitBrowser.newContext({...devices["iPhone 13"],baseURL,acceptDownloads:true});
  try {
    const mobile = await mobileContext.newPage();
    expect((await mobile.request.post("/_konae/owner/login",{data:{email:owner.displayName,password:"Owner-test-password-2026"}})).ok()).toBe(true);
    let providerRequests = 0;
    let claimAttempts = 0;
    let details = 0;
    let blobReads = 0;
    const pageStats: Array<{items:number;bytes:number;delayMs:number}> = [];
    mobile.on("request",(request)=>{
      const url=new URL(request.url());
      if(/api\.(?:anthropic|openai)\.com|openrouter\.ai/.test(url.hostname)) providerRequests++;
      if(url.pathname.endsWith("/claim")) claimAttempts++;
      if(/\/records\/[^/]+\/details$/.test(url.pathname)) details++;
      if(url.pathname.includes("/blobs/")) blobReads++;
    });
    await mobile.route(/https?:\/\/.+\/_konae\/projects\/.+\/records(?:\?.*)?$/,async route=>{
      const start=performance.now();
      const upstream=await route.fetch();
      const body=await upstream.body();
      const items=JSON.parse(body.toString()).items;
      await new Promise(resolve=>setTimeout(resolve,120+body.byteLength*8/10_000));
      pageStats.push({items:items.length,bytes:body.byteLength,delayMs:performance.now()-start});
      await route.fulfill({response:upstream,body});
    });

    await mobile.goto(projectPath+"?agent=durable-recovery");
    await showSection(mobile,"Describe");
    const history = mobile.getByRole("button",{name:/read saved conversations|conversation history|^history$/i});
    await history.focus();
    await expect(history).toBeFocused();
    await history.press("Enter");
    const dialog = mobile.getByRole("dialog",{name:"Conversation history"});
    await expect(dialog).toBeVisible();
    const row = dialog.getByRole("button",{name:/History fixture 1,000 entries/});
    await expect(row).toBeVisible();
    await row.evaluate((button, marker) => {
      button.addEventListener("click", () => {
        const start = performance.now();
        const timeline = button.closest("[role=dialog]")!;
        const listener = new MutationObserver(() => {
          if (!timeline.textContent?.includes(marker)) return;
          listener.disconnect();
          document.documentElement.dataset.firstReadableKonaeMs = String(performance.now() - start);
        });
        listener.observe(timeline, { childList: true, subtree: true, characterData: true });
      }, { once:true });
    }, inertLabel);
    await row.click();
    await expect(dialog.getByText(inertLabel,{exact:true})).toBeVisible();
    const readableMs=Math.round(Number(await mobile.locator("html").getAttribute("data-first-readable-konae-ms")));
    expect(readableMs).toBeGreaterThan(120);
    expect(await dialog.locator("b",{hasText:"Konae fixture markup stays as text"}).count()).toBe(0);
    expect(pageStats).toHaveLength(1);
    expect(pageStats[0].items).toBeLessThanOrEqual(50);
    expect(details).toBe(0);
    expect(blobReads).toBe(0);
    expect(providerRequests).toBe(0);
    expect(claimAttempts).toBe(0);
    expect(await dialog.getByText(oldestLabel,{exact:true}).count()).toBe(0);

    await dialog.getByRole("button",{name:/Load older entries/i}).click();
    await expect.poll(()=>pageStats.length).toBe(2);
    expect(pageStats[1].items).toBeLessThanOrEqual(50);
    expect(blobReads).toBe(0);
    const tool = dialog.locator(".durable-timeline-entry").filter({hasText:/pi\.tool-result/}).last();
    await tool.getByText(/Expand.*(result|trace)/i).click();
    await expect(dialog.getByText(modelTail,{exact:false}).last()).toBeVisible();
    const explicit = dialog.getByRole("button",{name:/Load original result/i});
    if (await explicit.count()) await explicit.click();
    await expect(dialog.getByText(originalTail,{exact:false}).last()).toBeVisible();
    expect(details).toBe(1);
    expect(blobReads).toBe(1);
    expect(providerRequests).toBe(0);
    expect(claimAttempts).toBe(0);
    const layout = await dialog.evaluate(element=>({width:element.getBoundingClientRect().width,viewport:window.innerWidth}));
    expect(layout.width).toBeLessThanOrEqual(layout.viewport);
    console.log(JSON.stringify({mobile:"WebKit iPhone 13",firstReadableMs:readableMs,pageStats,blobReads,providerRequests,layout}));

    const download = mobile.waitForEvent("download");
    await dialog.getByRole("button",{name:/Download JSON/i}).click();
    const artifact = await download;
    expect(artifact.suggestedFilename()).toContain(fixture.conversationId);
    const content = JSON.parse(await readFile((await artifact.path())!,"utf8"));
    expect(content.version).toBe(1);
    expect(content.blobs?.some((entry: {digest:string})=>entry.digest===fixture.digest)).toBe(true);
    await dialog.getByRole("button",{name:"Delete",exact:true}).click();
    await dialog.getByRole("button",{name:"Delete conversation",exact:true}).click();
    await expect(row).toHaveCount(0);
    expect((await page.request.get(fixture.path)).status()).toBe(404);
  } finally {
    await mobileContext.close();
    await webkitBrowser.close();
  }
});

test("desktop proposal can be read without a key and explicitly continued and applied on mobile WebKit", async ({page,baseURL}) => {
  const projectPath=await createProject(page,"Proposal travels to WebKit","tasks");
  const projectId=projectPath.split("/").at(-1)!;
  const address="/_konae/projects/"+projectId+"/conversations";
  const desktopFixture=await installProposalFixture(page);
  desktopFixture.setScenario("long-read");
  const initial="Read the large scratch file and prepare the exact cross-device proposal proof.";
  await page.goto(projectPath+"?agent=durable-recovery");
  await connectProposalFixture(page,"fixture-desktop-separate-key");
  await askFixture(page,initial);
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading",{name:"PROPOSAL PREVIEW"})).toBeVisible({timeout:45000});
  await expect(page.getByRole("button",{name:"Apply",exact:true})).toBeEnabled();
  const { items }=await (await page.request.get(address)).json();
  expect(items).toHaveLength(1);
  const conversation=address+"/"+items[0].id;
  const initialRevision=(await (await page.request.get(conversation+"/checkpoints")).json()).items[0].payload.sourcesRevision;
  const owner=await (await page.request.get("/_konae/owner/session")).json();
  const phoneBrowser=await webkit.launch({ channel: undefined, args: [], timeout: 15_000 });
  const phoneContext=await phoneBrowser.newContext({...devices["iPhone 13"],baseURL});
  try {
    const phone=await phoneContext.newPage();
    const auth=await phone.request.post("/_konae/owner/login",{data:{email:owner.displayName,password:"Owner-test-password-2026"}});
    expect(auth.ok()).toBe(true);
    let phoneProvider=0;
    let providerWithOwnKey=0;
    let phoneClaimAttempts=0;
    const wireBodies: unknown[]=[];
    phone.on("request",request=>{
      if(new URL(request.url()).pathname.endsWith("/claim")) phoneClaimAttempts++;
    });
    await phone.route("https://api.anthropic.com/**",async route=>{
      const request=route.request();
      const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-headers":"*","access-control-allow-methods":"POST,OPTIONS"};
      if (request.method()==="OPTIONS") return route.fulfill({status:204,headers});
      phoneProvider++;
      if(request.headers()["x-api-key"]==="fixture-phone-separate-key") providerWithOwnKey++;
      wireBodies.push(request.postDataJSON());
      await route.fulfill({status:200,contentType:"text/event-stream",headers,body:providerReply(undefined,"I see the restored proposal was saved from this phone.")});
    });
    const measurements: Array<{method:string;route:string;incoming:number;outgoing:number;delayMs:number}> = [];
    let mobileProfile = false;
    await phone.route(url => url.origin === new URL(baseURL!).origin && (url.pathname === "/mcp" || url.pathname.startsWith("/_konae/")),async route=>{
      if (!mobileProfile) return await route.continue();
      const start=performance.now();
      const request=route.request();
      const payload=request.postDataBuffer();
      const outgoing=payload?.byteLength ?? 0;
      await new Promise(resolve=>setTimeout(resolve,outgoing*8/2_000));
      const upstream=await route.fetch();
      const body=await upstream.body();
      const incoming=body.byteLength;
      await new Promise(resolve=>setTimeout(resolve,120+incoming*8/10_000));
      const pathname=new URL(request.url()).pathname;
      measurements.push({method:request.method(),route:pathname.split("/").at(-1)!,incoming,outgoing,delayMs:performance.now()-start});
      await route.fulfill({response:upstream,body});
    });
    await phone.goto(projectPath+"?agent=durable-recovery");
    await showSection(phone,"Describe");
    const history=phone.getByRole("button",{name:/read saved conversations|conversation history|^history$/i});
    const dialog=phone.getByRole("dialog",{name:"Conversation history"});
    const chooseConversation=async()=>{
      await history.click();
      await expect(dialog).toBeVisible();
      await dialog.locator(".durable-conversation-choice").first().click();
      await expect(dialog.locator(".durable-timeline-entry").first()).toBeVisible();
      await expect(dialog.getByText(initial,{exact:true})).toBeVisible();
    };
    await chooseConversation();
    expect(phoneProvider).toBe(0);
    await dialog.getByRole("button",{name:"Inspect candidate files",exact:true}).click();
    const candidate=dialog.getByRole("region",{name:"Read-only candidate comparison"});
    await expect(candidate).toBeVisible();
    await expect(candidate.locator("pre").filter({hasText:"PROPOSAL PREVIEW"})).toHaveCount(1);
    expect(phoneProvider).toBe(0);
    expect(phoneClaimAttempts).toBe(0);
    const readEntry=dialog.locator(".durable-timeline-entry").filter({hasText:/pi\.tool-result/}).filter({has:phone.getByText(/toolResult.*read/i)}).last();
    await readEntry.locator("summary").click();
    await expect(readEntry.getByText(/cut at 20000 characters/i).last()).toBeVisible();
    await expect(readEntry.getByText(actualReadTail,{exact:false})).toHaveCount(0);
    await readEntry.getByRole("button",{name:/Load original result/i}).click();
    await expect(readEntry.getByText(actualReadTail,{exact:false}).last()).toBeVisible();
    expect(phoneProvider).toBe(0);
    const leaseState=async()=> (await (await phone.request.get(conversation+"/lease")).json());
    expect((await leaseState()).live).toBe(true);
    await dialog.getByRole("button",{name:/Ask it to stop and release/i}).click();
    await expect.poll(async()=>(await leaseState()).live,{timeout:32000}).toBe(false);
    const returnedGeneration=(await leaseState()).generation;
    expect(phoneProvider).toBe(0);
    const continueInHistory=dialog.getByRole("button",{name:/Continue from this saved conversation/i});
    await expect(continueInHistory).toBeVisible({timeout:6000});
    await continueInHistory.click();
    const phoneKeys=phone.getByRole("dialog",{name:"Connect a model"});
    await phoneKeys.getByLabel("Anthropic API key").fill("fixture-phone-separate-key");
    await phoneKeys.getByRole("button",{name:"Save",exact:true}).click();
    await expect(phoneKeys).toBeHidden();
    expect((await leaseState()).generation).toBe(returnedGeneration);
    expect((await leaseState()).live).toBe(false);
    expect(phoneProvider).toBe(0);

    if (!(await dialog.isVisible())) await chooseConversation();
    const started=performance.now();
    mobileProfile=true;
    await dialog.getByRole("button",{name:/Continue from this saved conversation/i}).click();
    await expect(dialog).toBeHidden({timeout:35000});
    await showSection(phone,"Preview");
    await expect(phone.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading",{name:"PROPOSAL PREVIEW"})).toBeVisible({timeout:45000});
    await showSection(phone,"Describe");
    await expect(phone.getByRole("button",{name:"Apply",exact:true})).toBeEnabled();
    expect((await leaseState()).generation).toBeGreaterThan(returnedGeneration);
    const readinessMs=Math.round(performance.now()-started);
    mobileProfile=false;
    console.log(JSON.stringify({profile:"same-origin Konae API: 120ms RTT, 2Mbps up/10Mbps down simulated per route",readinessMs,totalRx:measurements.reduce((size,m)=>size+m.incoming,0),totalTx:measurements.reduce((size,m)=>size+m.outgoing,0),requests:measurements}));
    await phone.getByRole("button",{name:"Apply",exact:true}).click();
    await expect(phone.getByText(/Applied .* to the project\./)).toBeVisible({timeout:35000});
    const checkpoints=async()=> (await (await phone.request.get(conversation+"/checkpoints")).json()).items;
    await expect.poll(async()=> (await checkpoints())[0].payload.status).toBe("saved");
    expect((await checkpoints())[0].payload.sourcesRevision).toBe(initialRevision+1);
    expect((await readSavedSources(phone,projectId)).files["index.html"]).toContain("PROPOSAL PREVIEW");

    await askFixture(phone,"Confirm this saved proposal from the matching earlier context.");
    await expect(phone.getByText("I see the restored proposal was saved from this phone.",{exact:true})).toBeVisible({timeout:35000});
    expect(phoneProvider).toBeGreaterThan(0);
    expect(providerWithOwnKey).toBe(phoneProvider);
    expect(JSON.stringify(wireBodies)).toContain(initial);
    expect(JSON.stringify(wireBodies)).toContain("PROPOSAL PREVIEW");
    expect(JSON.stringify(wireBodies)).toContain("The prior proposal was saved and read back from the installation");
    console.log(JSON.stringify({device:"iPhone 13 WebKit",continueToReadyPreviewMs:readinessMs,network:"loopback with throttled Konae API",samePersistedProject:true,interceptedProviderKeyDeviceOnly:true}));
  } finally {
    await phoneContext.close();
    await phoneBrowser.close();
  }
});

test("late timeline and detail responses cannot update a different conversation", async ({ page, baseURL }) => {
  const path = await createProject(page, "Owner history request identity");
  const projectId = path.split("/").at(-1)!;
  const origin = new URL(baseURL!).origin;
  const cookie = (await page.context().cookies()).map(({name,value})=>name+"="+value).join("; ");
  const fetcher = (input: string, init?: RequestInit) => {
    const headers = new Headers(init?.headers);
    headers.set("cookie", cookie);
    headers.set("origin", origin);
    return fetch(new URL(input,origin),{...init,headers});
  };
  const a = await seedHistory({ projectId, fetcher, historyCount: 80, title: "First history", label: "First history's record", prefix: "First conversation only" });
  const b = await seedHistory({ projectId, fetcher, historyCount: 80, title: "Second history", label: "Second history's record", recordOffset: 2000, prefix: "Second conversation only" });
  expect(a.toolEntryId).not.toBe(b.toolEntryId);
  await page.goto(path + "?agent=durable-recovery");
  await showSection(page, "Describe");
  await page.getByRole("button", { name: /^Read saved conversations$/i }).click();
  const history = page.getByRole("dialog", { name: "Conversation history" });
  await history.getByRole("button", { name: /First history/ }).click();
  await expect(history.getByText("First history's record", {exact:true})).toBeVisible();
  let allowDetails!: () => void;
  const held = new Promise<void>(resolve => { allowDetails = resolve; });
  await page.route(url => url.pathname === a.path + "/records/" + a.toolEntryId + "/details", async route => {
    const upstream = await route.fetch();
    await held;
    await route.fulfill({ response: upstream });
  });
  let allowOlder!: () => void;
  const heldOlder = new Promise<void>(resolve => { allowOlder = resolve; });
  await page.route(url => url.pathname === a.path + "/records" && url.searchParams.has("before"), async route => {
    const upstream = await route.fetch();
    await heldOlder;
    await route.fulfill({ response:upstream });
  });
  const aOlder = page.waitForRequest(request=>new URL(request.url()).pathname === a.path + "/records" && new URL(request.url()).searchParams.has("before"));
  const aDetailRequest = page.waitForRequest(request=>new URL(request.url()).pathname === a.path + "/records/" + a.toolEntryId + "/details");
  const details = () => history.locator(".durable-timeline-entry").filter({hasText:/pi\.tool-result/}).last().locator("summary");
  try {
    await details().click();
    await aDetailRequest;
    await history.getByRole("button", {name:/Load older entries/i}).click();
    await aOlder;
    await history.getByRole("button", { name: /Second history/ }).click();
    await expect(history.getByText("Second history's record", {exact:true})).toBeVisible();
    const returnedA = page.waitForResponse(response=>new URL(response.url()).pathname === a.path + "/records/" + a.toolEntryId + "/details");
    allowDetails();
    await returnedA;
    const returnedOlder = page.waitForResponse(response=>new URL(response.url()).pathname === a.path + "/records" && new URL(response.url()).searchParams.has("before"));
    allowOlder();
    await returnedOlder;
    const bDetail = page.waitForRequest(request=>new URL(request.url()).pathname === b.path + "/records/" + b.toolEntryId + "/details",{timeout:3000});
    await details().click();
    await bDetail;
    await expect(history.getByText(modelTail,{exact:false}).last()).toBeVisible();
    await expect(history.getByText("First history's record", {exact:true})).toHaveCount(0);
    await expect(history.getByText(/First conversation only/)).toHaveCount(0);
  } finally {
    allowDetails();
    allowOlder();
  }
});

test("local attachment copies a checkpoint only after confirmation from the owner side", async ({ page }) => {
  const path = await createProject(page, "Local attachment target");
  const projectId = path.split("/").at(-1)!;
  const endpoint = "/_konae/projects/" + projectId + "/conversations";
  const savedBefore = await readSavedSources(page,projectId);
  const fixture = await installProposalFixture(page);
  fixture.setScenario("drift");
  const writes: Array<{path:string;body:string}> = [];
  page.on("request", request => {
    const url = new URL(request.url());
    if ((url.pathname.includes("/conversations") || url.pathname === "/mcp") && request.method() !== "GET") {
      writes.push({path:url.pathname,body:request.postData() ?? ""});
    }
  });
  await page.goto("/local");
  await page.getByRole("button",{name:"Model key",exact:true}).click();
  const settings = page.getByRole("dialog",{name:"Connect a model"});
  const keyMarker="local-attachment-fixture-key-unused";
  await settings.getByLabel("Anthropic API key").fill(keyMarker);
  await settings.getByRole("button",{name:"Save",exact:true}).click();
  const prompt="Keep this local proof in the tab until I confirm attachment.";
  await askFixture(page,prompt);
  await expect(page.frameLocator('iframe[title="Local-only app preview"]').getByRole("heading",{name:"PROPOSAL PREVIEW"})).toBeVisible({timeout:40000});
  await page.getByRole("button",{name:"Sign in to attach a copy",exact:true}).click();
  const ownerLink=page.getByRole("link",{name:/back to your projects|open your projects/i});
  if (await ownerLink.isVisible({timeout:3000})) await ownerLink.click();
  if (new URL(page.url()).pathname.startsWith("/_konae/owner/login")) await page.goto("/");
  await expect(await page.request.get(endpoint)).toBeOK();
  expect((await (await page.request.get(endpoint)).json()).items).toHaveLength(0);
  const attach = page.getByRole("dialog",{name:"Review local attachment"});
  await expect(attach).toBeVisible();
  await attach.getByLabel("Owned Project").selectOption(projectId);
  let commits=0;
  let omittedReadback=false;
  await page.route(url => url.pathname.startsWith(endpoint + "/") && url.pathname.endsWith("/commit"),async route=>{
    const response=await route.fetch();
    if (response.ok() && (await response.json()).stored===1) commits++;
    await route.fulfill({response});
  });
  await page.route(url => url.pathname.startsWith(endpoint + "/") && url.pathname.endsWith("/records") && url.searchParams.has("idsOnly"),async route=>{
    if (commits===2 && !omittedReadback) {
      omittedReadback=true;
      await route.abort();
    } else await route.continue();
  });
  const attachButton=attach.getByRole("button",{name:"Attach this copy",exact:true});
  await attachButton.click();
  await expect(attach.getByRole("alert")).toBeVisible({timeout:25000});
  expect(omittedReadback).toBe(true);
  expect(commits).toBe(2);
  await attachButton.click();
  await expect(attach).toBeHidden({timeout:35000});
  expect(commits).toBe(2);
  const items=(await (await page.request.get(endpoint)).json()).items;
  expect(items).toHaveLength(1);
  const attached=endpoint+"/"+items[0].id;
  const exported=await (await page.request.get(attached+"/download")).json();
  const exportedText=JSON.stringify(exported);
  expect(exportedText).toContain(prompt);
  expect(exportedText).not.toContain(keyMarker);
  expect(exported.blobs.some((blob:{base64:string})=>Buffer.from(blob.base64,"base64").toString().includes("PROPOSAL PREVIEW"))).toBe(true);
  expect((await readSavedSources(page,projectId)).files).toEqual(savedBefore.files);
  expect(writes.filter(item=>item.path===endpoint)).toHaveLength(1);
  expect(writes.some(item=>item.body.includes(keyMarker))).toBe(false);
});

test("switching owners clears only that owner's cached workspace and execution identities", async ({ page }) => {
  const active = await (await page.request.get("/_konae/owner/session")).json();
  expect(active.signedIn).toBe(true);
  await page.goto("/");
  await expect(page.getByRole("heading",{name:"Projects",exact:true})).toBeVisible();
  const user = encodeURIComponent(active.ownerId);
  const keys = [`konae:owner:${user}:workspace-fixture`, `konae:durable-recovery:${user}:project-fixture`, `konae:durable-executor:${user}:project-fixture`];
  const unrelated = ["konae:owner:unrelated-fixture:workspace-fixture", "unrelated-application-storage"];
  await page.evaluate(({keys,unrelated})=> [...keys,...unrelated].forEach(key=>localStorage.setItem(key,"fixture-value")),{keys,unrelated});
  const newOwner = await signupOwner(page,"203.0.113.99",`alternate-${crypto.randomUUID()}@example.test`);
  expect(newOwner.ok).toBe(true);
  await page.reload();
  await expect(page.getByRole("heading",{name:"Projects",exact:true})).toBeVisible();
  expect(await page.evaluate(keys=>keys.map(key=>localStorage.getItem(key)),keys)).toEqual([null,null,null]);
  expect(await page.evaluate(keys=>keys.map(key=>localStorage.getItem(key)),unrelated)).toEqual(["fixture-value","fixture-value"]);
});

test("a queued autosave leaves the saved files of a newly opened project intact", async ({ page, browser, baseURL }) => {
  const app = await createProject(page, "Queued autosave source fixture");
  const projectId = app.split("/").at(-1)!;
  const before = await readSavedSources(page, projectId);
  expect(before.files["index.html"]).toBeTruthy();
  const owner = await (await page.request.get("/_konae/owner/session")).json();
  const secondary = await browser.newContext({ baseURL });
  try {
    const viewer = await secondary.newPage();
    const login = await viewer.request.post("/_konae/owner/login", {
      data: { email: owner.displayName, password: "Owner-test-password-2026" },
    });
    expect(login.ok()).toBe(true);
    await viewer.addInitScript(() => {
      const queued: (() => void)[] = [];
      Object.defineProperty(window, "__testQueuedSourceAutosaves", { value: queued });
      const schedule = window.setTimeout.bind(window);
      window.setTimeout = (callback, delay, ...args) => {
        if (delay === 500 && typeof callback === "function" && callback.toString().includes(".hasSources(")) {
          queued.push(() => callback());
        }
        return schedule(callback, delay, ...args);
      };
    });
    await viewer.goto(app + "?agent=durable-recovery");
    await expect(viewer.frameLocator("iframe").getByRole("heading", { name: "Tiny Tasks" })).toBeVisible({ timeout: 35_000 });
    const write = viewer.waitForResponse((response) => {
      const request = response.request();
      return new URL(response.url()).pathname === "/mcp" && (request.postData() ?? "").includes('"project_sources_save"');
    }, { timeout: 5_000 }).catch(() => null);
    await viewer.evaluate(() => {
      const queued = (window as Window & { __testQueuedSourceAutosaves?: (() => void)[] }).__testQueuedSourceAutosaves;
      if (!queued?.[0]) throw new Error("The editor did not schedule its initial source autosave.");
      // Run the queued callback with its earlier project list after the
      // source loader marks the project ready.
      queued[0]();
    });
    expect(await write).toBeNull();
    expect(await readSavedSources(page, projectId)).toEqual(before);
  } finally {
    await secondary.close();
  }
});

test("a standby that loses admission retains its unsent text, without making a provider request", async ({ page, browser, baseURL }) => {
  const projectPath=await createProject(page,"One shared writer with unsent standby");
  const projectId=projectPath.split("/").at(-1)!;
  const address="/_konae/projects/"+projectId+"/conversations";
  const owner=await (await page.request.get("/_konae/owner/session")).json();
  const standby=await browser.newContext({baseURL});
  let releaseProvider!:()=>void;
  const held=new Promise<void>(resolve=>{releaseProvider=resolve;});
  let countStandbyProvider=0;
  try {
    await page.route("https://api.anthropic.com/**",async route=>{
      const request=route.request();
      const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-headers":"*","access-control-allow-methods":"POST,OPTIONS"};
      if (request.method()==="OPTIONS") return route.fulfill({status:204,headers});
      await held;
      await route.fulfill({status:200,contentType:"text/event-stream",headers,body:providerReply(undefined,"First browser finishes.")}).catch(()=>{});
    });
    const standbyPage=await standby.newPage();
    expect((await standbyPage.request.post("/_konae/owner/login",{data:{email:owner.displayName,password:"Owner-test-password-2026"}})).ok()).toBe(true);
    await standbyPage.route("https://api.anthropic.com/**",async route=>{ if(route.request().method()==="POST") countStandbyProvider++; await route.abort(); });
    await page.goto(projectPath+"?agent=durable-recovery");
    await standbyPage.goto(projectPath+"?agent=durable-recovery");
    await connectProposalFixture(page,"writer-fixture-key");
    await connectProposalFixture(standbyPage,"standby-fixture-key");
    const providerPosted=page.waitForRequest(req=>req.method()==="POST"&&new URL(req.url()).hostname==="api.anthropic.com", {timeout:35000});
    await askFixture(page,"Hold the first submitted request in its admitted generation.");
    await providerPosted;
    let before: Array<{id:string;lastSeq:number}> = [];
    let previousSequence = -1;
    let stableReads = 0;
    for (let attempt=0; attempt<12 && stableReads<2; attempt++) {
      before=(await (await page.request.get(address)).json()).items;
      expect(before).toHaveLength(1);
      if (before[0].lastSeq === previousSequence) stableReads++;
      else { previousSequence=before[0].lastSeq; stableReads=0; }
      if (stableReads<2) await page.waitForTimeout(250);
    }
    expect(stableReads).toBeGreaterThanOrEqual(2);
    const draft="Standby preserves this unsent draft when a live writer already holds the lease.";
    await askFixture(standbyPage,draft);
    await expect(standbyPage.getByText(/live execution lease/i).first()).toBeVisible({timeout:16000});
    expect(countStandbyProvider).toBe(0);
    await expect(standbyPage.getByRole("textbox",{name:"Message to agent"})).toHaveValue(draft,{timeout:3000});
    const after=(await (await page.request.get(address)).json()).items;
    expect(after).toHaveLength(1);
    const active=after.find((item:{id:string})=>item.id===before[0].id);
    // The admitted browser is still allowed to persist its in-flight turn while
    // the standby is refused. The standby contract is zero provider traffic and
    // no second conversation, not a frozen sequence for the active writer.
    expect(active.lastSeq).toBeGreaterThanOrEqual(before[0].lastSeq);
  } finally {
    releaseProvider();
    await standby.close();
  }
});

test("provider error evidence redacts configured keys and sensitive URL parameters before persistence", async ({ page }) => {
  const path=await createProject(page,"Redacted provider error fixture");
  const projectId=path.split("/").at(-1)!;
  const ownerPrefix="/_konae/projects/"+projectId+"/conversations";
  const localKey="synthetic-current-provider-credential-unused";
  const urlToken="fixture-secret-in-access-query";
  const clientSecret="fixture-private-oauth-client-secret";
  const invitationCode="fixture-private-one-use-invitation";
  const marker="synthetic provider refuses the request";
  const sameOriginRequests:string[]=[];
  page.on("request",req=>{
    const url=new URL(req.url());
    if(url.pathname==="/mcp"||url.pathname.startsWith("/_konae/projects/")) sameOriginRequests.push(req.postData()??"");
  });
  let providerPosts=0;
  await page.route("https://api.anthropic.com/**",async route=>{
    const request=route.request();
    const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-methods":"POST,OPTIONS","access-control-allow-headers":"*"};
    if(request.method()==="OPTIONS") return route.fulfill({status:204,headers});
    providerPosts++;
    expect(request.headers()["x-api-key"]).toBe(localKey);
    await route.fulfill({status:400,headers,contentType:"application/json",body:JSON.stringify({
      type:"error",
      error:{type:"invalid_request_error",message:marker+" "+localKey+" https://fixture.invalid/continue?access_token="+urlToken+"&client_secret="+clientSecret+"&reason=denied http://fixture.invalid/_konae/invitation#invitation="+invitationCode},
    })});
  });
  await page.goto(path+"?agent=durable-recovery");
  await connectProposalFixture(page,localKey);
  await askFixture(page,"Show an ordinary refusal without copying private credentials.");
  await expect.poll(()=>providerPosts,{timeout:20000}).toBe(1);
  await expect(page.getByText("model_error",{exact:true})).toBeVisible({timeout:35000});
  const {items}=await(await page.request.get(ownerPrefix)).json();
  expect(items).toHaveLength(1);
  const endpoint=ownerPrefix+"/"+items[0].id;
  const download=await(await page.request.get(endpoint+"/download")).json();
  const exported=JSON.stringify(download);
  expect(exported).toContain(marker);
  expect(exported).toContain("reason=denied");
  for (const credential of [localKey, urlToken, clientSecret, invitationCode]) {
    expect(exported).not.toContain(credential);
    expect(sameOriginRequests.some((body) => body.includes(credential))).toBe(false);
  }
});

test("an offline active browser keeps its unsent prompt and preserves the last acknowledged turn", async ({ page }) => {
  const app=await createProject(page,"Keep active browser draft during an outage");
  const projectId=app.split("/").at(-1)!;
  const conversations="/_konae/projects/"+projectId+"/conversations";
  let providerPosts=0;
  await page.route("https://api.anthropic.com/**",async route=>{
    const request=route.request();
    const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-methods":"POST,OPTIONS","access-control-allow-headers":"*"};
    if(request.method()==="OPTIONS") return route.fulfill({status:204,headers});
    providerPosts++;
    await route.fulfill({status:200,headers,contentType:"text/event-stream",body:providerReply(undefined,"The first fixture turn is saved.")});
  });
  await page.goto(app+"?agent=durable-recovery");
  await connectProposalFixture(page,"offline-fixture-key");
  await askFixture(page,"Store the coherent first turn on the installation.");
  await expect(page.getByText("The first fixture turn is saved.",{exact:true})).toBeVisible({timeout:30000});
  await expect(page.getByRole("button",{name:"Stop",exact:true})).toHaveCount(0);
  const {items}=await(await page.request.get(conversations)).json();
  expect(items).toHaveLength(1);
  const before=items[0].lastSeq;
  expect(before).toBeGreaterThan(0);
  const draft="Save this unsent text locally during the fixture network outage.";
  await page.context().setOffline(true);
  try {
    await askFixture(page,draft);
    await expect(page.getByRole("button",{name:"Stop",exact:true})).toHaveCount(0);
    expect(providerPosts).toBe(1);
    await expect(page.getByRole("textbox",{name:"Message to agent"})).toHaveValue(draft,{timeout:3000});
  } finally { await page.context().setOffline(false); }
  const after=await(await page.request.get(conversations+"/"+items[0].id)).json();
  expect(after.lastSeq).toBe(before);
});

test("legacy rollback retains an owner reader and starts a separate model context from saved sources", async ({ page }) => {
  const app=await createProject(page,"Rollback reads existing owner history");
  const projectId=app.split("/").at(-1)!;
  const endpoint="/_konae/projects/"+projectId+"/conversations";
  const savedSourceMarker="SAVED_SOURCE_BASELINE_FOR_LEGACY_FIXTURE";
  const editor=await openFile(page,"index.html");
  const starter=await editor.inputValue();
  await editor.fill(starter+"<p>"+savedSourceMarker+"</p>");
  await waitForKernelSave(page);
  const durablePrompt="DURABLE_TRANSCRIPT_PROVENANCE_FIXTURE";
  let providerBody="";
  let mode:"durable"|"legacy"="durable";
  await page.route("https://api.anthropic.com/**",async route=>{
    const request=route.request();
    const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-methods":"POST,OPTIONS","access-control-allow-headers":"*"};
    if(request.method()==="OPTIONS") return route.fulfill({status:204,headers});
    if(mode==="legacy") providerBody=request.postData()??"";
    await route.fulfill({status:200,headers,contentType:"text/event-stream",body:providerReply(undefined,mode==="durable"?"Durable fixture answer.":"Separate legacy fixture answer.")});
  });
  await page.goto(app+"?agent=durable-recovery");
  await connectProposalFixture(page,"rollback-fixture-local-key");
  await askFixture(page,durablePrompt);
  await expect(page.getByText("Durable fixture answer.",{exact:true})).toBeVisible({timeout:30000});
  await expect(page.getByRole("button",{name:"Stop",exact:true})).toHaveCount(0);
  const {items}=await(await page.request.get(endpoint)).json();
  expect(items).toHaveLength(1);
  const hold=endpoint+"/"+items[0].id;
  const seq=items[0].lastSeq;
  expect(seq).toBeGreaterThan(0);

  mode="legacy";
  await page.goto(app+"?agent=legacy");
  await showSection(page,"Describe");
  await page.getByRole("button",{name:"Read saved conversations",exact:true}).click({timeout:8000});
  const history=page.getByRole("dialog",{name:"Conversation history"});
  await history.locator(".durable-conversation-choice").first().click();
  await expect(history.getByText(durablePrompt,{exact:true})).toBeVisible();
  if (await history.getByRole("button",{name:/Continue from this saved conversation/i}).count()) {
    await expect(history.getByRole("button",{name:/Continue from this saved conversation/i})).toBeDisabled();
  }
  await history.getByRole("button",{name:"Close Conversation history",exact:true}).click();
  expect(providerBody).toBe("");
  await askFixture(page,"LEGACY_FRESH_INPUT_FIXTURE");
  await expect(page.getByText("Separate legacy fixture answer.",{exact:true})).toBeVisible({timeout:35000});
  expect(providerBody).toContain(savedSourceMarker);
  expect(providerBody).toContain("LEGACY_FRESH_INPUT_FIXTURE");
  expect(providerBody).not.toContain(durablePrompt);
  expect((await(await page.request.get(hold)).json()).lastSeq).toBe(seq);
});


test("confirmed fixture publish advances the Durable baseline before the provider result is checkpointed", async ({ page }) => {
  const app=await createProject(page,"Confirm a local fixture Deployment","tasks");
  const projectId=app.split("/").at(-1)!;
  await page.goto(app+"?agent=durable-recovery");
  await connectProposalFixture(page,"deployment-fixture-browser-key");
  const fixture=await installProposalFixture(page);
  fixture.setScenario("drift");
  await askFixture(page,"Prepare the file proposal for the local Deployment fixture.");
  const preview=page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(preview.getByRole("heading",{name:"PROPOSAL PREVIEW",exact:true})).toBeVisible({timeout:35000});
  await expect(page.getByRole("button",{name:"Apply",exact:true})).toBeEnabled();
  await page.getByRole("button",{name:"Apply",exact:true}).click();
  await expect(page.getByText(/Applied .* to the project/)).toBeVisible({timeout:25000});
  let releaseWrites=0;
  let projectReadsAfterWrite=0;
  page.on("request",request=>{
    if(new URL(request.url()).pathname!=="/mcp") return;
    let payload;try{payload=request.postDataJSON();}catch{return;}
    const name=payload?.params?.name;
    if(name==="release_apply") releaseWrites++;
    if(name==="project_inspect"&&releaseWrites>0) projectReadsAfterWrite++;
  });
  await page.unroute("https://api.anthropic.com/**");
  let step=0;
  await page.route("https://api.anthropic.com/**",async route=>{
    const request=route.request();
    const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-methods":"POST,OPTIONS","access-control-allow-headers":"*"};
    if(request.method()==="OPTIONS") return route.fulfill({status:204,headers});
    const tool=step++===0?{name:"publish",input:{}}:undefined;
    await route.fulfill({status:200,headers,contentType:"text/event-stream",body:providerReply(tool,"Konae observed the confirmed fixture Deployment.")});
  });
  await askFixture(page,"Open your existing Deploy dialog for this local test app.");
  const dialog=page.getByRole("dialog").filter({has:page.getByRole("button",{name:/^Deploy (public|changes)$/})});
  await expect(dialog).toBeVisible({timeout:25000});
  expect(releaseWrites).toBe(0);
  const visibility=dialog.getByRole("radiogroup",{name:"Deployment visibility"});
  if(await visibility.count()) await visibility.getByText("Public",{exact:true}).click();
  await dialog.getByRole("button",{name:/^Deploy (public|changes)$/}).click();
  await expect(page.locator("a.published-url")).toBeVisible({timeout:35000});
  await expect(page.getByText("Konae observed the confirmed fixture Deployment.",{exact:true})).toBeVisible({timeout:30000});
  await expect(page.getByRole("button",{name:"Done",exact:true})).toBeEnabled();
  await page.getByRole("button",{name:"Done",exact:true}).click();
  expect(releaseWrites).toBe(1);
  expect(projectReadsAfterWrite).toBeGreaterThan(0);
  const cookie=(await page.context().cookies()).map(({name,value})=>name+"="+value).join("; ");
  const mcp=new McpClient(new URL("/mcp",page.url()).href,{fetch:(url,init)=>fetch(url,{...init,headers:{...Object.fromEntries(new Headers(init?.headers)),cookie}})});
  const saved=await mcp.call<{revision:number;transfer:Transfer}>("project_sources_read",{projectId,requestId:crypto.randomUUID()});
  const stored=JSON.parse(new TextDecoder().decode(await mcp.download(saved.transfer)));
  expect(stored.files["index.html"]).toContain("PROPOSAL PREVIEW");
  const conversations=(await(await page.request.get(`/_konae/projects/${projectId}/conversations`)).json()).items;
  expect(conversations).toHaveLength(1);
  const latest=await(await page.request.get(`/_konae/projects/${projectId}/conversations/${conversations[0].id}/checkpoints?latest=true`)).json();
  expect(latest.items).toHaveLength(1);
  expect(latest.items[0].payload.status).toBe("saved");
  expect(latest.items[0].payload.sourcesRevision).toBe(saved.revision);
  expect(await page.getByText(/SessionFailed/).count()).toBe(0);
});

test("only an explicitly declared objective appears in history, linked to its saved message", async ({ page }) => {
  const app = await createProject(page, "Declared objective fixture");
  const projectId = app.split("/").at(-1)!;
  const path = `/_konae/projects/${projectId}/conversations`;
  const providerKey = "owner-intent-synthetic-provider-key";
  const pastedPassword = "fixture-only-pasted-password-marker";
  const objective = "Keep the fixture app portable";
  const otherPrompt = "Use the existing template and explain the result";
  const providerBodies: string[] = [];
  const backendBodies: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith(path)) backendBodies.push(request.postData() ?? "");
  });
  await page.route("https://api.anthropic.com/**", async (route) => {
    const request = route.request();
    const headers = {
      "access-control-allow-origin": request.headers().origin ?? "*",
      "access-control-allow-methods": "POST,OPTIONS",
      "access-control-allow-headers": "*",
    };
    if (request.method() === "OPTIONS") return route.fulfill({ status: 204, headers });
    expect(request.headers()["x-api-key"]).toBe(providerKey);
    providerBodies.push(request.postData() ?? "");
    await route.fulfill({
      status: 200, headers, contentType: "text/event-stream",
      body: providerReply(undefined, `Answer to synthetic turn ${providerBodies.length}`),
    });
  });
  await page.goto(app + "?agent=durable-recovery");
  await connectProposalFixture(page, providerKey);
  const visiblePrompt = `Objective: ${objective} ${providerKey} Password=${pastedPassword}\n\nUse the current checklist.`;
  await askFixture(page, visiblePrompt);
  await expect(page.getByText("Answer to synthetic turn 1", { exact: true })).toBeVisible({ timeout: 35_000 });
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
  await askFixture(page, otherPrompt);
  await expect(page.getByText("Answer to synthetic turn 2", { exact: true })).toBeVisible({ timeout: 35_000 });
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);

  const list = await (await page.request.get(path)).json();
  expect(list.items).toHaveLength(1);
  const conversation = `${path}/${list.items[0].id}`;
  type Declaration = { origin: string; sourceEntryId: string; text: string; declaredAt: number };
  const timeline: { items: Array<{ id: number; summary: { kind: string; intent?: Declaration } }> } =
    await (await page.request.get(conversation + "/records")).json();
  const ownerMessages = timeline.items.filter((row) => row.summary.kind === "konae.owner-message");
  expect(ownerMessages).toHaveLength(2);
  const declarations = ownerMessages.filter((row) => row.summary.intent);
  expect(declarations).toHaveLength(1);
  const [declared] = declarations;
  const safeObjective = `${objective} [redacted credential] Password=[redacted]`;
  expect(declared.summary.intent).toMatchObject({ origin: "owner-message", sourceEntryId: String(declared.id), text: safeObjective });
  expect(declared.summary.intent?.declaredAt).toBeGreaterThan(0);
  expect(providerBodies[0]).toContain(`Objective: ${safeObjective}\\n\\nUse the current checklist.`);
  expect(providerBodies[1]).toContain(otherPrompt);

  const downloaded = JSON.stringify(await (await page.request.get(conversation + "/download")).json());
  expect(downloaded).toContain(safeObjective);
  for (const evidence of [downloaded, providerBodies.join("\n"), backendBodies.join("\n")]) {
    expect(evidence).not.toContain(providerKey);
    expect(evidence).not.toContain(pastedPassword);
  }
  await page.getByRole("button", { name: "Read saved conversations", exact: true }).click();
  const history = page.getByRole("dialog", { name: "Conversation history" });
  await history.locator(".durable-conversation-choice").first().click();
  await expect(history.getByText("Declared intent", { exact: true })).toHaveCount(1);
  await expect(history.locator(".durable-intent pre")).toContainText(String(declared.id));
  expect(providerBodies).toHaveLength(2);
});

test("one staged browser can advance and the expired browser's late provider message is fenced", async ({ page, browser }) => {
  const projectPath = await createProject(page, "Owner browser lease race", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const address = "/_konae/projects/" + projectId + "/conversations";
  let notifyRequest: () => void;
  let releaseResponse: () => void;
  const providerStarted = new Promise<void>((resolve) => { notifyRequest = resolve; });
  const hold = new Promise<void>((resolve) => { releaseResponse = resolve; });
  let providerPosts = 0;
  const lateContent = "<main>STALE PROVIDER FIXTURE MARKER</main>";
  await page.route("https://api.anthropic.com/**", async (route) => {
    const request = route.request();
    const origin = request.headers().origin ?? "*";
    const headers = { "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-allow-methods": "POST,OPTIONS" };
    if (request.method() === "OPTIONS") return route.fulfill({status:204,headers});
    providerPosts++;
    notifyRequest();
    await hold;
    await route.fulfill({status:200,headers,contentType:"text/event-stream",body:providerReply({ name:"write",input:{path:"/workspace/index.html",content:lateContent} })}).catch(() => {});
  });

  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page);
  await askFixture(page, "Hold the actual fixture generation request after durable admission.");
  await Promise.race([providerStarted,new Promise<void>((_resolve,reject) => setTimeout(()=>reject(new Error("staged generation was not sent")),35000))]);
  const inventory = await (await page.request.get(address)).json();
  expect(inventory.items).toHaveLength(1);
  const id = inventory.items[0].id;
  const conversation = address + "/" + id;
  const getLease = async (from: Page) => {
    const result = await from.request.get(conversation + "/lease");
    expect(result.ok()).toBe(true);
    return result.json();
  };
  const original = await getLease(page);
  expect(original.live).toBe(true);
  expect(original.generation).toBe(1);
  expect(inventory.items[0].lastSeq).toBeGreaterThan(0);
  const originalEntries = await (await page.request.get(conversation+"/batches?after=0&limit=50")).json();
  expect(JSON.stringify(originalEntries)).toContain("Hold the actual fixture generation request");

  const firstOwner = await (await page.request.get("/_konae/owner/session")).json();
  const secondContext = await browser.newContext({baseURL:new URL(page.url()).origin});
  let standbyProviderRequests = 0;
  try {
    const standby = await secondContext.newPage();
    const login = await standby.request.post("/_konae/owner/login",{data:{email:firstOwner.displayName,password:"Owner-test-password-2026"}});
    expect(login.ok(),"Isolated second owner login").toBe(true);
    await standby.route("https://api.anthropic.com/**",async route=>{
      const request=route.request();
      const headers={"access-control-allow-origin":request.headers().origin??"*","access-control-allow-headers":"*","access-control-allow-methods":"POST,OPTIONS"};
      if (request.method()==="OPTIONS") return route.fulfill({status:204,headers});
      standbyProviderRequests++;
      await route.fulfill({status:200,headers,contentType:"text/event-stream",body:providerReply(undefined,"Standby reviewed the now-authorized fixture.")});
    });
    await standby.goto(projectPath+"?agent=durable-recovery&conversation="+id);
    await expect(standby.getByRole("navigation",{name:"Workspace sections"})).toBeVisible();
    await showSection(standby,"Describe");
    expect((await getLease(standby)).generation).toBe(1);
    expect(standbyProviderRequests).toBe(0);
    const continueButton = standby.getByRole("button",{name:"Continue from saved conversation",exact:true});
    await standby.getByRole("button",{name:"Read saved conversations",exact:true}).click();
    const passiveHistory=standby.getByRole("dialog",{name:"Conversation history"});
    await passiveHistory.locator(".durable-conversation-choice").first().click();
    await expect(passiveHistory.getByRole("button",{name:/Ask it to stop and release/i})).toBeVisible();
    await expect(passiveHistory.getByRole("button",{name:/Continue from this saved conversation/i})).toHaveCount(0);
    await passiveHistory.getByRole("button",{name:"Close Conversation history",exact:true}).click();
    expect((await getLease(standby)).generation).toBe(1);
    expect(standbyProviderRequests).toBe(0);

    const root = process.env.KONAE_E2E_DATA_DIR;
    expect(root).toContain("konae-e2e-");
    const metadata = new DatabaseSync(path.join(root!, "mcp.sqlite"));
    try {
      expect(metadata.prepare("UPDATE conversation_execution_leases SET expires_at=0 WHERE project=?").run(projectId).changes).toBe(1);
    } finally { metadata.close(); }
    await connectProposalFixture(standby);
    await continueButton.click();
    await expect.poll(async()=> (await getLease(standby)).generation,{timeout:35000}).toBe(2);
    expect((await getLease(standby)).executorId).not.toBe(original.executorId);
    releaseResponse!();
    await expect(page.getByRole("button",{name:"Stop",exact:true})).toHaveCount(0,{timeout:30000});
    const contents = (await (await standby.request.get(conversation+"/batches?after=0&limit=50")).text());
    expect(contents).not.toContain("STALE PROVIDER FIXTURE MARKER");
    expect(providerPosts).toBe(1);

    const delayedStop = await page.request.post(conversation+"/stop",{data:{requestId:"fixture-delayed-stop",generation:1}});
    expect(delayedStop.status()).toBe(409);
    expect((await getLease(standby)).live).toBe(true);
    const handoff = await page.request.post(conversation+"/stop",{data:{requestId:"fixture-cooperative-stop",generation:2}});
    expect(handoff.ok()).toBe(true);
    const requested = await handoff.json();
    expect(requested.generation).toBe(2);
    await expect.poll(async()=> (await getLease(standby)).live,{timeout:30000}).toBe(false);
    expect((await getLease(standby)).generation).toBe(2);
    const status = (await getLease(standby));
    expect(status.stopRequested).toBe(false);
  } finally {
    releaseResponse!();
    await secondContext.close();
  }
});

test("Durable staged editor reconciles interrupted Apply before claiming the expired project lease", async ({ page }) => {
  const projectPath = await createProject(page, "Durable interrupted Apply", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const fixture = await installProposalFixture(page);
  let providerPosts = 0;
  page.on("request", (req) => { if (req.method() === "POST" && req.url().startsWith("https://api.anthropic.com/")) providerPosts++; });
  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page);
  fixture.setScenario("drift");
  await askFixture(page, "Prepare and show the exact checked candidate.");
  const iframe = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(iframe.getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 50000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();

  const base = "/_konae/projects/" + projectId + "/conversations";
  const { items } = await (await page.request.get(base)).json();
  expect(items).toHaveLength(1);
  const conversation = base + "/" + items[0].id;
  const getCheckpoints = async () => (await (await page.request.get(conversation + "/checkpoints")).json()).items;

  let committed = 0;
  let blockedReadback = 0;
  let loseReplies = true;
  await page.route("**/mcp", async (r) => {
    const body = r.request().postDataJSON();
    if (!loseReplies) {
      if (body?.params?.name === "project_sources_save") committed++;
      return r.continue();
    }
    if (body?.params?.name === "project_sources_save") {
      committed++;
      const ack = await r.fetch();
      expect(ack.ok()).toBe(true);
      expect((await ack.json()).result.isError).not.toBe(true);
      return r.abort("connectionreset");
    }
    if (committed && body?.params?.name === "project_sources_read") {
      blockedReadback++;
      return r.abort("connectionreset");
    }
    return r.continue();
  });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/Apply outcome is unobserved/).first()).toBeVisible({ timeout: 35000 });
  expect(committed).toBe(1);
  expect(blockedReadback).toBeGreaterThan(0);
  expect(["applied-unsaved", "apply-unknown"]).toContain((await getCheckpoints())[0].payload.status);
  await expect(page.getByText(/so nothing was applied/)).toHaveCount(0);
  const getEffects = async () => (await (await page.request.get(conversation+"/effects")).json()).items;
  const beforeEffects = await getEffects();
  const sourceEffect = beforeEffects.find((item: { tool: string; state: string }) => item.tool==="project_sources_save" && item.state==="unknown");
  expect(sourceEffect).toBeTruthy();
  loseReplies = false;
  const metadata = new DatabaseSync(path.join(process.env.KONAE_E2E_DATA_DIR!, "mcp.sqlite"));
  try {
    expect(metadata.prepare("UPDATE conversation_execution_leases SET expires_at=0 WHERE project=?").run(projectId).changes).toBe(1);
  } finally { metadata.close(); }
  const before = providerPosts;
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Describe");
  await page.getByRole("button", { name: "Continue from saved conversation", exact: true }).click();
  await expect.poll(async () => (await getCheckpoints())[0].payload.status, { timeout: 35000 }).toBe("saved");
  expect((await readSavedSources(page, projectId)).files["index.html"]).toContain("PROPOSAL PREVIEW");
  expect(providerPosts).toBe(before);
  expect(committed).toBe(1);
  const afterEffect = (await getEffects()).find((item: { callId: string }) => item.callId===sourceEffect.callId);
  expect(afterEffect?.state).toBe("observed");
  expect(afterEffect?.observationCallId).toBeTruthy();
});


test("Durable staged editor observes a lost successful Apply response and keeps the binary", async ({ page }) => {
  const projectPath = await createProject(page, "Durable source read-back", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const retainedAsset = await seedRetainedBinary(page, projectId);
  let providerPosts = 0;
  page.on("request", (request) => { if (request.method() === "POST" && request.url().startsWith("https://api.anthropic.com/")) providerPosts++; });
  const fixture = await installProposalFixture(page);
  await page.goto(projectPath + "?agent=durable-recovery");
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await connectProposalFixture(page);
  fixture.setScenario("drift");
  await askFixture(page, "Prepare the exact checked fixture proposal.");
  const iframe = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(iframe.getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 50000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  const route = "/_konae/projects/" + projectId + "/conversations";
  const items = await (await page.request.get(route)).json();
  expect(items.items).toHaveLength(1);
  const conversation = route + "/" + items.items[0].id;
  const listCheckpoints = async () => (await (await page.request.get(conversation + "/checkpoints")).json()).items;
  let checks = await listCheckpoints();
  expect(checks[0].payload.status).toBe("proposal");
  const manifest = checks[0].payload.scratch.manifest;
  expect(manifest["public/retained.png"].encoding).toBe("base64");
  expect(checks.some((item) => item.payload.status === "working")).toBe(true);

  const count = providerPosts;
  expect(count).toBeGreaterThan(0);
  // This test reloads an idle tab to inspect a lost source-save reply. Release
  // its real lease first, as a crashed device would otherwise wait for expiry.
  const lease = await (await page.request.get(conversation + "/lease")).json();
  expect(lease.live).toBe(true);
  const released = await page.request.post(conversation + "/release", { data: { generation: lease.generation, executorId: lease.executorId } });
  expect(released.ok()).toBe(true);
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Describe");
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Continue from saved conversation", exact: true }).click();
  await expect(iframe.getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 50000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  await expect(page.getByText(/Test rows, uploaded test files, and actor sessions were reset/)).toBeVisible();
  expect(providerPosts).toBe(count);

  let acceptedWrites = 0;
  let readsAfterLoss = 0;
  await page.route("**/mcp", async (r) => {
    const body = r.request().postDataJSON();
    if (body?.params?.name !== "project_sources_save") {
      if (acceptedWrites && body?.params?.name === "project_sources_read") readsAfterLoss++;
      return r.continue();
    }
    acceptedWrites++;
    const response = await r.fetch();
    const result = await response.json();
    expect(response.status()).toBe(200);
    expect(result.result.isError).not.toBe(true);
    await r.abort("connectionreset");
  });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/Applied .* to the project\./)).toBeVisible({ timeout: 40000 });
  const saved = await readSavedSources(page, projectId);
  expect(saved.files["index.html"]).toContain("PROPOSAL PREVIEW");
  expect(saved.files["public/retained.png"]).toBe(retainedAsset);
  expect(saved.encodings?.["public/retained.png"]).toBe("base64");
  checks = await listCheckpoints();
  expect(checks[0].payload.status).toBe("saved");
  expect(checks.some((item) => item.payload.status === "applied-unsaved")).toBe(true);
  expect(acceptedWrites).toBe(1);
  expect(readsAfterLoss).toBeGreaterThanOrEqual(1);
});

test("Durable staged editor reconciles Apply on reopen when both replies were unavailable", async ({ page }) => {
  const projectPath = await createProject(page, "Durable interrupted Apply", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const fixture = await installProposalFixture(page);
  let providerPosts = 0;
  page.on("request", (req) => { if (req.method() === "POST" && req.url().startsWith("https://api.anthropic.com/")) providerPosts++; });
  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page);
  fixture.setScenario("drift");
  await askFixture(page, "Prepare and show the exact checked candidate.");
  const iframe = page.frameLocator('iframe[title="Mini-app preview"]');
  await expect(iframe.getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 50000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();

  const base = "/_konae/projects/" + projectId + "/conversations";
  const { items } = await (await page.request.get(base)).json();
  expect(items).toHaveLength(1);
  const conversation = base + "/" + items[0].id;
  const getCheckpoints = async () => (await (await page.request.get(conversation + "/checkpoints")).json()).items;

  let committed = 0;
  let blockedReadback = 0;
  let loseReplies = true;
  await page.route("**/mcp", async (r) => {
    const body = r.request().postDataJSON();
    if (!loseReplies) {
      if (body?.params?.name === "project_sources_save") committed++;
      return r.continue();
    }
    if (body?.params?.name === "project_sources_save") {
      committed++;
      const ack = await r.fetch();
      expect(ack.ok()).toBe(true);
      expect((await ack.json()).result.isError).not.toBe(true);
      return r.abort("connectionreset");
    }
    if (committed && body?.params?.name === "project_sources_read") {
      blockedReadback++;
      return r.abort("connectionreset");
    }
    return r.continue();
  });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/Apply outcome is unobserved/).first()).toBeVisible({ timeout: 35000 });
  expect(committed).toBe(1);
  expect(blockedReadback).toBeGreaterThan(0);
  expect(["applied-unsaved", "apply-unknown"]).toContain((await getCheckpoints())[0].payload.status);
  await expect(page.getByText(/so nothing was applied/)).toHaveCount(0);
  loseReplies = false;
  const before = providerPosts;
  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Describe");
  await page.getByRole("button", { name: "Continue from saved conversation", exact: true }).click();
  await expect.poll(async () => (await getCheckpoints())[0].payload.status, { timeout: 35000 }).toBe("saved");
  expect((await readSavedSources(page, projectId)).files["index.html"]).toContain("PROPOSAL PREVIEW");
  expect(providerPosts).toBe(before);
  expect(committed).toBe(1);
});

test("Durable staged stale proposal stays inspectable and only a deliberate new attempt branches", async ({ page }) => {
  const projectPath = await createProject(page, "Durable stale source fixture", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const fixture = await installProposalFixture(page);
  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page);
  fixture.setScenario("drift");
  await askFixture(page, "Draft changes for the stale-source fixture.");
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 48000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  const list = "/_konae/projects/" + projectId + "/conversations";
  const conversations = async () => (await (await page.request.get(list)).json()).items;
  const initial = await conversations();
  expect(initial).toHaveLength(1);
  const oldPath = list + "/" + initial[0].id;
  const readCheckpoint = async (url: string) => (await (await page.request.get(url + "/checkpoints")).json()).items[0];
  const oldCheckpoint = await readCheckpoint(oldPath);
  expect(oldCheckpoint.payload.status).toBe("proposal");

  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => name + "=" + value).join("; ");
  const other = new McpClient(new URL("/mcp", page.url()).href, {
    fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }),
  });
  const saved = await other.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const original = JSON.parse(new TextDecoder().decode(await other.download(saved.transfer)));
  original.files["index.html"] += "<p>Concurrent owner source save</p>";
  const uploaded = await other.upload(new TextEncoder().encode(JSON.stringify(original)), projectId);
  await other.call("project_sources_save", { projectId, transferId: uploaded.transferId, expectedRevision: saved.revision });
  const lease = (await (await page.request.get(oldPath + "/lease")).json());
  if (lease.live) {
    expect((await page.request.post(oldPath+"/stop",{data:{requestId:crypto.randomUUID(),generation:lease.generation}})).ok()).toBe(true);
    await expect.poll(async()=> (await (await page.request.get(oldPath+"/lease")).json()).live,{timeout:35000}).toBe(false);
  }

  await page.reload();
  await expect(page.getByRole("navigation", { name: "Workspace sections" })).toBeVisible();
  await showSection(page, "Describe");
  await page.getByRole("button", { name: "Continue from saved conversation", exact: true }).click();
  const oldDiff = page.getByRole("region", { name: "Preserved stale proposal" });
  await expect(oldDiff.getByRole("heading", { name: "Saved sources changed" })).toBeVisible();
  await oldDiff.getByText(/Inspect preserved proposal/).click();
  await expect(oldDiff.getByRole("heading", { name: "index.html", exact: true })).toBeVisible();
  await expect(oldDiff.locator("pre").filter({ hasText: "PROPOSAL PREVIEW" })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toHaveCount(0);
  expect(await conversations()).toHaveLength(1);
  expect((await readSavedSources(page, projectId)).files["index.html"]).toContain("Concurrent owner source save");
  const preserved = await readCheckpoint(oldPath);
  expect(preserved.payload.status).toBe("proposal");
  await oldDiff.getByRole("button", { name: "Start linked attempt from current saved sources", exact: true }).click();
  await expect(oldDiff).toHaveCount(0);
  await askFixture(page, "Try a fresh checked candidate based on the new saved source.");
  await expect.poll(async () => (await conversations()).length, { timeout: 40000 }).toBe(2);
  const created = (await conversations()).find((conversation) => conversation.id !== initial[0].id);
  await expect.poll(async () => (await readCheckpoint(list + "/" + created.id))?.payload.parentCheckpointId, { timeout: 35000 }).toBe(preserved.checkpointId);
  expect(await conversations()).toHaveLength(2);
  expect((await readCheckpoint(oldPath)).checkpointId).toBe(preserved.checkpointId);
  expect((await readSavedSources(page, projectId)).files["index.html"]).toContain("Concurrent owner source save");
});

test("restoring a selected Durable checkpoint creates a linked candidate from exact context and fences stale Apply", async ({ page }) => {
  const projectPath = await createProject(page, "Durable checkpoint restore fixture", "tasks");
  const projectId = projectPath.split("/").at(-1)!;
  const fixture = await installProposalFixture(page);
  fixture.setScenario("drift");
  const originalPrompt = "Prepare the original checkpoint restore proposal.";
  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page, "checkpoint-restore-browser-key");
  let providerRequests: Array<Record<string, unknown>> = [];
  page.on("request", (request) => {
    if (new URL(request.url()).origin === "https://api.anthropic.com" && request.method() === "POST") {
      try { providerRequests.push(request.postDataJSON()); } catch { /* only JSON provider calls are relevant */ }
    }
  });
  await askFixture(page, originalPrompt);
  await expect(page.frameLocator('iframe[title="Mini-app preview"]').getByRole("heading", { name: "PROPOSAL PREVIEW" })).toBeVisible({ timeout: 45000 });
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  const list = `/_konae/projects/${projectId}/conversations`;
  const conversations = async () => (await (await page.request.get(list)).json()).items;
  let parent = (await conversations())[0];
  const parentPath = `${list}/${parent.id}`;
  const parentCheckpoints = (await (await page.request.get(`${parentPath}/checkpoints`)).json()).items;
  const selected = parentCheckpoints.find((item: any) => item.payload.status === "proposal");
  expect(selected).toBeTruthy();
  const dialogOpen = page.getByRole("button", { name: /Read saved conversations|Conversation history|^History$/i });
  await dialogOpen.click();
  const history = page.getByRole("dialog", { name: "Conversation history" });
  let fullCheckpointListReads = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname === `${parentPath}/checkpoints` && !url.search) fullCheckpointListReads++;
  });
  await history.locator(".durable-conversation-choice").filter({ hasText: parent.title }).click();
  await expect(history.getByText(originalPrompt, { exact: true })).toBeVisible();
  const stop = history.getByRole("button", { name: "Ask it to stop and release", exact: true });
  if (await stop.count()) {
    await stop.click();
    await expect.poll(async () => (await (await page.request.get(`${parentPath}/lease`)).json()).live).toBe(false);
    parent = await (await page.request.get(parentPath)).json();
  }
  expect(fullCheckpointListReads).toBe(0);
  await history.getByRole("button", { name: "Browse earlier checkpoints", exact: true }).click();
  await expect.poll(() => fullCheckpointListReads).toBe(1);
  await history.getByRole("button", { name: new RegExp(`proposal · sequence ${selected.sequence}`) }).click();
  let finishDelayedCheckpoint!: () => void;
  let delayedCheckpointRequested!: () => void;
  const checkpointRequested = new Promise<void>((resolve) => { delayedCheckpointRequested = resolve; });
  const delayCheckpoint = new Promise<void>((resolve) => { finishDelayedCheckpoint = resolve; });
  const selectedCheckpointUrl = new RegExp(`/_konae/projects/${projectId}/conversations/${parent.id}/checkpoints\\?checkpoint_id=${selected.checkpointId}`);
  await page.route(selectedCheckpointUrl, async (route) => {
    const response = await route.fetch();
    delayedCheckpointRequested();
    await delayCheckpoint;
    await route.fulfill({ response });
  });
  await history.getByRole("button", { name: "Inspect candidate files", exact: true }).click();
  await checkpointRequested;
  const otherCheckpoint = parentCheckpoints.find((item: any) => item.sequence !== selected.sequence);
  expect(otherCheckpoint).toBeTruthy();
  await history.getByRole("button", { name: new RegExp(`${otherCheckpoint.payload.status} · sequence ${otherCheckpoint.sequence}`) }).click();
  finishDelayedCheckpoint();
  await page.unroute(selectedCheckpointUrl);
  await expect(history.getByRole("region", { name: "Read-only candidate comparison" })).toHaveCount(0);
  await expect(history.getByRole("button", { name: "Restore as a new proposal from this checkpoint", exact: true })).toHaveCount(0);
  await history.getByRole("button", { name: new RegExp(`proposal · sequence ${selected.sequence}`) }).click();
  await history.getByRole("button", { name: "Inspect candidate files", exact: true }).click();
  await expect(history.getByRole("region", { name: "Read-only candidate comparison" })).toBeVisible();
  const providerCountBeforeRestore = providerRequests.length;
  await history.getByRole("button", { name: "Restore as a new proposal from this checkpoint", exact: true }).click();
  await expect(history).toHaveCount(0);
  const originalSources = await readSavedSources(page, projectId);
  expect(originalSources.files["index.html"]).not.toContain("PROPOSAL PREVIEW");
  await expect(page.getByRole("region", { name: "Restored proposal diff" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Restored proposal diff" })).toContainText(`current saved source base at revision ${originalSources.revision}`);
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();
  expect(providerRequests).toHaveLength(providerCountBeforeRestore);

  const created = (await conversations()).find((item: any) => item.id !== parent.id);
  expect(created).toBeTruthy();
  const branchPath = `${list}/${created.id}`;
  await expect.poll(async () => (await (await page.request.get(`${branchPath}/checkpoints`)).json()).items[0]?.payload.parentCheckpointId).toBe(selected.checkpointId);
  const branchCheckpoints = (await (await page.request.get(`${branchPath}/checkpoints`)).json()).items;
  expect(branchCheckpoints[0].payload.parentConversationId).toBe(parent.id);
  expect(branchCheckpoints[0].payload.restoreProvenance).toEqual({ parentConversationId: parent.id, parentCheckpointId: selected.checkpointId, parentSequence: selected.sequence });
  expect((await (await page.request.get(parentPath)).json()).lastSeq).toBe(parent.lastSeq);

  fixture.setScenario("context");
  await askFixture(page, "Continue from the restored proposal and explain the current candidate.");
  await expect.poll(() => providerRequests.length).toBeGreaterThan(providerCountBeforeRestore);
  const wire = JSON.stringify(providerRequests.at(-1));
  expect(wire).toContain(originalPrompt);
  expect(wire).toContain("PROPOSAL PREVIEW");
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled();

  await dialogOpen.click();
  await history.locator(".durable-conversation-choice").filter({ hasText: parent.title }).click();
  await history.getByRole("button", { name: "Browse earlier checkpoints", exact: true }).click();
  const originalChoice = history.getByRole("button", { name: new RegExp(`proposal · sequence ${selected.sequence}$`) });
  await expect(originalChoice).toBeVisible();
  await history.locator(".durable-conversation-choice").filter({ hasText: created.title }).click();
  await expect(history.getByRole("button", { name: "Browse earlier checkpoints", exact: true })).toBeVisible();
  await expect(originalChoice).toHaveCount(0);
  await history.getByRole("button", { name: "Close Conversation history", exact: true }).click();

  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const mcp = new McpClient(new URL("/mcp", page.url()).href, { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
  const saved = await mcp.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const incoming = JSON.parse(new TextDecoder().decode(await mcp.download(saved.transfer)));
  incoming.files["index.html"] += "\n<!-- owner changed source after restore -->";
  const transfer = await mcp.upload(new TextEncoder().encode(JSON.stringify(incoming)), projectId);
  await mcp.call("project_sources_save", { projectId, transferId: transfer.transferId, expectedRevision: saved.revision });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/source.*changed|stale|current saved sources/i).last()).toBeVisible({ timeout: 30000 });
  const current = await readSavedSources(page, projectId);
  expect(current.files["index.html"]).toContain("owner changed source after restore");
  expect((await (await page.request.get(parentPath)).json()).id).toBe(parent.id);
});

test("restore before schema proposal keeps deployed rows and invitation, exposes provenance, and Redo forks the same checkpoint", async ({ page }) => {
  const projectPath = await createProject(page, "Restore before schema and invite", "notes");
  const projectId = projectPath.split("/").at(-1)!;
  const fixture = await installProposalFixture(page);
  // Establish the local fixture deployment before durable conversation work.
  await page.getByRole("banner").getByRole("button", { name: /^Deploy/ }).click();
  const deploy = page.getByRole("dialog");
  await expect(deploy).toBeVisible();
  await deploy.getByRole("button", { name: "Deploy private", exact: true }).click();
  await expect(page.locator("a.published-url")).toBeVisible({ timeout: 35000 });
  await page.getByRole("button", { name: "Done", exact: true }).click();
  const originalAsset = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x00, 0xff]).toString("base64");
  const retainedAsset = Buffer.from([0x00, 0xfe, 0x01, 0x80]).toString("base64");
  const currentReplacement = Buffer.from([0xff, 0x52, 0x45, 0x50, 0x4c]).toString("base64");
  const seededSources = await readSavedSources(page, projectId);
  seededSources.files["public/restored.bin"] = originalAsset;
  seededSources.files["public/retained.bin"] = retainedAsset;
  seededSources.encodings = { ...seededSources.encodings, "public/restored.bin": "base64", "public/retained.bin": "base64" };
  await saveFixtureSources(page, projectId, seededSources.revision, seededSources);
  const cookies = await page.context().cookies();
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join("; ");
  const mcp = new McpClient(new URL("/mcp", page.url()).href, { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
  const session = await mcp.call<{ can?: Record<string, boolean> }>("session_inspect", {});
  expect(session.can?.manage_users).toBe(true);
  expect((await mcp.listTools()).some((tool) => tool.name === "app_invitations_list")).toBe(true);
  fixture.setScenario("schema");
  const beforeSchemaPrompt = "Add the small indexed schema improvement requested for this app.";
  await page.goto(projectPath + "?agent=durable-recovery");
  await connectProposalFixture(page, "schema-invitation-restore-key");
  let modelPostCount = 0;
  page.on("request", (request) => { if (new URL(request.url()).origin === "https://api.anthropic.com" && request.method() === "POST") modelPostCount++; });
  await askFixture(page, beforeSchemaPrompt);
  await expect(page.getByRole("button", { name: "Apply", exact: true })).toBeEnabled({ timeout: 45000 });
  const inviteEmail = `restore-${crypto.randomUUID()}@example.test`;
  fixture.setScenario("invite"); fixture.setInviteEmail(inviteEmail);
  await askFixture(page, `Invite ${inviteEmail} to this app and read the invitation back.`);
  const allow = page.getByRole("dialog", { name: "Allow the agent to do this?" });
  await expect(allow).toBeVisible({ timeout: 25000 });
  await allow.getByRole("button", { name: "Allow", exact: true }).click();
  const conversationListPath = `/_konae/projects/${projectId}/conversations`;
  const parentForInvite = (await (await page.request.get(conversationListPath)).json()).items[0];
  const effectsPath = `${conversationListPath}/${parentForInvite.id}/effects`;
  await expect.poll(async () => {
    const items = (await (await page.request.get(effectsPath)).json()).items;
    const inviteEffect = items.find((item: any) => item.tool === "app_invitations_create");
    return inviteEffect?.state ?? null;
  }, { timeout: 35000 }).toBe("observed");
  const inviteReceipt = (await (await page.request.get(effectsPath)).json()).items.find((item: any) => item.tool === "app_invitations_create");
  expect(inviteReceipt.state).toBe("observed");
  expect(inviteReceipt.observationCallId).toBeTruthy();
  const invitationBefore = await mcp.call<{ invitations: Array<{ invitationId: string; email: string; status: string }> }>("app_invitations_list", { projectId });
  const invitation = invitationBefore.invitations.find((item) => item.email === inviteEmail);
  expect(invitation?.invitationId).toBeTruthy();
  expect(invitation?.status).toBe("pending");
  await mcp.call("database_execute", { projectId, operation: "sql", input: { sql: "INSERT INTO notes(owner_id,title,created_at) VALUES ('fixture-actor','live row before restore',1)" } });
  const rowsBefore = await mcp.call<{ rows: unknown[] }>("database_execute", { projectId, operation: "sql", input: { sql: "SELECT id,owner_id,title,created_at FROM notes WHERE title = 'live row before restore' LIMIT 10" } });
  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/Applied .* to the project/)).toBeVisible({ timeout: 25000 });
  const currentSchema = await readSavedSources(page, projectId);
  expect(currentSchema.files["schema.sql"]).toContain("notes_owner_created_at");
  const changedSources = await readSavedSources(page, projectId);
  changedSources.files["public/restored.bin"] = currentReplacement;
  changedSources.files["public/current-only.bin"] = Buffer.from([0x41, 0xff, 0x42]).toString("base64");
  changedSources.encodings = { ...changedSources.encodings, "public/restored.bin": "base64", "public/retained.bin": "base64", "public/current-only.bin": "base64" };
  await saveFixtureSources(page, projectId, changedSources.revision, changedSources);

  await showSection(page, "Describe");
  await page.getByRole("button", { name: /Read saved conversations/ }).click();
  const history = page.getByRole("dialog", { name: "Conversation history" });
  await history.locator(".durable-conversation-choice").first().click();
  await expect(history.getByText(beforeSchemaPrompt, { exact: true })).toBeVisible();
  const parent = (await (await page.request.get(`/_konae/projects/${projectId}/conversations`)).json()).items[0];
  const parentPath = `/_konae/projects/${projectId}/conversations/${parent.id}`;
  const stop = history.getByRole("button", { name: "Ask it to stop and release", exact: true });
  if (await stop.count()) {
    await stop.click();
  }
  await expect.poll(async () => (await (await page.request.get(`${parentPath}/lease`)).json()).live).toBe(false);
  const parentFinal = await (await page.request.get(parentPath)).json();
  const archived = await (await page.request.get(`${parentPath}/download`)).json();
  expect(JSON.stringify(archived)).not.toContain("#invitation=");
  for (const blob of archived.blobs as Array<{ base64: string }>) {
    expect(Buffer.from(blob.base64, "base64").toString("utf8")).not.toContain("#invitation=");
  }
  const parentCheckpoints = (await (await page.request.get(`${parentPath}/checkpoints`)).json()).items;
  const beforeSchema = parentCheckpoints.filter((item: any) => item.payload.status === "working").sort((a: any, b: any) => a.sequence - b.sequence)[0];
  expect(beforeSchema).toBeTruthy();
  expect(parentFinal.lastSeq).toBeGreaterThan(beforeSchema.sequence);
  await history.getByRole("button", { name: "Browse earlier checkpoints", exact: true }).click();
  await history.getByRole("button", { name: new RegExp(`working · sequence ${beforeSchema.sequence}`) }).click();
  await history.getByRole("button", { name: "Inspect candidate files", exact: true }).click();
  await expect(history.getByRole("region", { name: "Read-only candidate comparison" }).getByText("notes_owner_created_at", { exact: false })).toHaveCount(0);
  await expect(history.getByRole("region", { name: "External effects after checkpoint" })).toContainText("app_invitations_create");
  await expect(history.getByRole("region", { name: "External effects after checkpoint" })).toContainText("read-back call");
  const providerRequestCount = modelPostCount;
  await history.getByRole("button", { name: "Restore as a new proposal from this checkpoint", exact: true }).click();
  await expect(history).toHaveCount(0);
  expect(modelPostCount).toBe(providerRequestCount);
  await expect(page.getByRole("region", { name: "Restored proposal diff" })).toContainText("schema.sql");
  await expect(page.getByRole("region", { name: "Restored proposal diff" })).toContainText("notes_owner_created_at");
  const rowsAfterRestore = await mcp.call<{ rows: unknown[] }>("database_execute", { projectId, operation: "sql", input: { sql: "SELECT id,owner_id,title,created_at FROM notes WHERE title = 'live row before restore' LIMIT 10" } });
  const invitationAfterRestore = await mcp.call<{ invitations: Array<{ invitationId: string; email: string; status: string }> }>("app_invitations_list", { projectId });
  expect(rowsAfterRestore.rows).toEqual(rowsBefore.rows);
  expect(invitationAfterRestore.invitations.find((item) => item.email === inviteEmail)).toMatchObject({ invitationId: invitation!.invitationId, status: "pending" });

  const list = `/_konae/projects/${projectId}/conversations`;
  const conversations = async () => (await (await page.request.get(list)).json()).items;
  const firstBranch = (await conversations()).find((item: any) => item.id !== parent.id);
  expect(firstBranch).toBeTruthy();
  const readLatestBranchCheckpoint = async (id: string) => (await (await page.request.get(`${list}/${id}/checkpoints`)).json()).items[0];
  await expect.poll(async () => (await readLatestBranchCheckpoint(firstBranch.id))?.payload.parentCheckpointId).toBe(beforeSchema.checkpointId);
  expect((await readLatestBranchCheckpoint(firstBranch.id)).payload.parentConversationId).toBe(parent.id);

  let providerRequests: Array<Record<string, any>> = [];
  page.on("request", (request) => {
    if (new URL(request.url()).origin === "https://api.anthropic.com" && request.method() === "POST") {
      try { providerRequests.push(request.postDataJSON()); } catch { /* only the model wire matters */ }
    }
  });
  fixture.setScenario("context");
  const readyCountBeforeContinue = await page.getByText("The checked change is ready.", { exact: true }).count();
  await askFixture(page, "Describe the restored schema candidate in one sentence.");
  await expect.poll(() => providerRequests.length).toBeGreaterThan(0);
  await expect.poll(() => page.getByText("The checked change is ready.", { exact: true }).count()).toBe(readyCountBeforeContinue + 1);
  const firstWire = providerRequests.at(-1)!;
  expect(JSON.stringify(firstWire.messages)).toContain(beforeSchemaPrompt);
  expect(JSON.stringify(firstWire.messages)).not.toContain(inviteEmail);
  expect(JSON.stringify(firstWire.messages)).not.toContain("app_invitations_create");
  await page.getByRole("button", { name: /Read saved conversations/ }).click();
  const beforeReloadHistory = page.getByRole("dialog", { name: "Conversation history" });
  await beforeReloadHistory.locator(".durable-conversation-choice").filter({ hasText: firstBranch.title }).click();
  await expect(beforeReloadHistory.getByText("Describe the restored schema candidate in one sentence.", { exact: true })).toBeVisible();
  const beforeReloadStop = beforeReloadHistory.getByRole("button", { name: "Ask it to stop and release", exact: true });
  if (await beforeReloadStop.count()) await beforeReloadStop.click();
  await expect.poll(async () => (await (await page.request.get(`${list}/${firstBranch.id}/lease`)).json()).live).toBe(false);
  await beforeReloadHistory.getByRole("button", { name: "Close Conversation history" }).click();
  const providerCountBeforeReload = providerRequests.length;
  await page.reload();
  await page.getByRole("button", { name: "Continue from saved conversation", exact: true }).click();
  await expect(page.getByRole("button", { name: /Redo from checkpoint/ })).toBeVisible({ timeout: 30000 });
  expect(providerRequests).toHaveLength(providerCountBeforeReload);
  await page.getByRole("button", { name: /Redo from checkpoint/ }).click();
  const redoHistory = page.getByRole("dialog", { name: "Conversation history" });
  await expect(redoHistory).toBeVisible();
  await redoHistory.locator(".durable-conversation-choice").filter({ hasText: firstBranch.title }).click();
  await expect(redoHistory.getByText("Describe the restored schema candidate in one sentence.", { exact: true })).toBeVisible();
  await expect(redoHistory.getByText(/Another browser holds the active writer lease/)).toBeVisible();
  const redoStop = redoHistory.getByRole("button", { name: "Ask it to stop and release", exact: true });
  if (await redoStop.count()) {
    await redoStop.click();
    await expect.poll(async () => (await (await page.request.get(`${parentPath}/lease`)).json()).live).toBe(false);
  }
  await redoHistory.getByRole("button", { name: "Close Conversation history" }).click();
  await page.getByRole("button", { name: /Redo from checkpoint/ }).click();
  await expect.poll(async () => (await conversations()).length).toBe(3);
  await expect(page.getByRole("region", { name: "Restored proposal diff" })).toBeVisible();
  const redoBranch = (await conversations()).find((item: any) => item.id !== parent.id && item.id !== firstBranch.id);
  expect(redoBranch).toBeTruthy();
  await expect.poll(async () => (await readLatestBranchCheckpoint(redoBranch.id))?.payload.parentCheckpointId).toBe(beforeSchema.checkpointId);
  expect((await readLatestBranchCheckpoint(redoBranch.id)).payload.parentConversationId).toBe(parent.id);
  expect((await readLatestBranchCheckpoint(redoBranch.id)).payload.restoreProvenance).toEqual({ parentConversationId: parent.id, parentCheckpointId: beforeSchema.checkpointId, parentSequence: beforeSchema.sequence });
  await askFixture(page, "Describe the restored schema candidate in one sentence.");
  await expect.poll(() => providerRequests.length).toBeGreaterThan(1);
  const redoWire = providerRequests.at(-1)!;
  expect(JSON.stringify(redoWire.messages)).toContain(beforeSchemaPrompt);
  expect(JSON.stringify(redoWire.messages)).not.toContain(inviteEmail);
  expect(JSON.stringify(redoWire.messages)).not.toContain("app_invitations_create");

  await page.getByRole("button", { name: "Apply", exact: true }).click();
  await expect(page.getByText(/Applied .* to the project/)).toBeVisible({ timeout: 25000 });
  const finalSources = await readSavedSources(page, projectId);
  expect(finalSources.files["schema.sql"]).not.toContain("notes_owner_created_at");
  expect(finalSources.files["public/restored.bin"]).toBe(originalAsset);
  expect(finalSources.files["public/retained.bin"]).toBe(retainedAsset);
  expect(finalSources.encodings?.["public/restored.bin"]).toBe("base64");
  expect(finalSources.encodings?.["public/retained.bin"]).toBe("base64");
  expect(finalSources.files["public/current-only.bin"]).toBeUndefined();
  expect((await mcp.call<{ invitations: Array<{ invitationId: string; email: string }> }>("app_invitations_list", { projectId })).invitations.find((item) => item.email === inviteEmail)?.invitationId).toBe(invitation!.invitationId);
  const rowsAfterApply = await mcp.call<{ rows: unknown[] }>("database_execute", { projectId, operation: "sql", input: { sql: "SELECT id,owner_id,title,created_at FROM notes WHERE title = 'live row before restore' LIMIT 10" } });
  expect(rowsAfterApply.rows).toEqual(rowsBefore.rows);
});

for (const mode of ["manual-staged", "legacy-tool"] as const) {
  test(`existing Deploy flow remains usable with ${mode}`, async ({ page }) => {
    const app = await createProject(page, `Local Deploy fixture ${mode}`);
    const projectId = app.split("/").at(-1)!;
    const manual = mode === "manual-staged";
    const conversationsPath = `/_konae/projects/${projectId}/conversations`;
    let providerRequests = 0;
    let releases = 0;
    page.on("request", (request) => {
      if (new URL(request.url()).pathname !== "/mcp" || !request.postData()) return;
      if (request.postDataJSON().params?.name === "release_apply") releases++;
    });
    await page.route("https://api.anthropic.com/**", async (route) => {
      const request = route.request();
      const headers = {
        "access-control-allow-origin": request.headers().origin ?? "*",
        "access-control-allow-methods": "POST,OPTIONS",
        "access-control-allow-headers": "*",
      };
      if (request.method() === "OPTIONS") return route.fulfill({ status: 204, headers });
      providerRequests++;
      const tool = !manual && providerRequests === 1 ? { name: "publish", input: {} } : undefined;
      await route.fulfill({
        status: 200, headers, contentType: "text/event-stream",
        body: providerReply(tool, `Deploy fixture model response ${providerRequests}`),
      });
    });
    await page.goto(app + (manual ? "?agent=durable-recovery" : "?agent=legacy"));
    await connectProposalFixture(page, "local-deploy-path-fixture-key");
    const saved = await readSavedSources(page, projectId);
    await askFixture(page, manual ? "Remember this conversation while I deploy." : "Open the existing Deploy dialog.");
    const list = async () => (await (await page.request.get(conversationsPath)).json()).items;
    let conversation: string | undefined;
    if (manual) {
      await expect(page.getByText("Deploy fixture model response 1", { exact: true })).toBeVisible({ timeout: 25000 });
      await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
      expect(await list()).toHaveLength(1);
      conversation = (await list())[0].id;
      await page.getByRole("banner").getByRole("button", { name: "Deploy", exact: true }).click();
    }
    const dialog = page.getByRole("dialog").filter({ has: page.getByRole("button", { name: "Deploy public", exact: true }) });
    await expect(dialog).toBeVisible({ timeout: 25000 });
    expect(releases).toBe(0);
    await dialog.getByRole("button", { name: "Deploy public", exact: true }).click();
    await expect(page.locator("a.published-url")).toBeVisible({ timeout: 30000 });
    await page.getByRole("button", { name: "Done", exact: true }).click();
    const published = await readSavedSources(page, projectId);
    expect(published.revision).toBeGreaterThan(saved.revision);
    expect(releases).toBe(1);
    if (manual) await askFixture(page, "Continue using the source baseline I just published.");
    await expect(page.getByText("Deploy fixture model response 2", { exact: true })).toBeVisible({ timeout: 20000 });
    expect(providerRequests).toBe(2);
    if (manual) {
      expect(await list()).toHaveLength(1);
      expect((await list())[0].id).toBe(conversation);
      const checkpoint = (await (await page.request.get(`${conversationsPath}/${conversation}/checkpoints?latest=true`)).json()).items[0];
      expect(checkpoint.payload.sourcesRevision).toBe(published.revision);
    } else expect(await list()).toHaveLength(0);
    expect(releases).toBe(1);
  });
}
