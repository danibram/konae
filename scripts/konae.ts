#!/usr/bin/env bun
import { chmod, cp, mkdir, mkdtemp, open, readFile, readdir, readlink, rename, rm, stat, symlink, writeFile } from "node:fs/promises";
import { existsSync, watch } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import * as ts from "typescript";
import { createInterface } from "node:readline/promises";
import { McpClient } from "../src/mcp/client";
import { packageComponentFiles, type PackageComponentName } from "../src/lib/portable-package";
import {
  DATA_FOLDER, DIST_FOLDER, KERNEL_VERSION, SETTINGS_FILE, SOURCES_FOLDER, dataEntries, generatedFiles, gitignore,
  type AppSettings, type FolderEntry,
} from "../src/lib/app-folder";
import { describeSchemaChange, publishProject, type ConfirmSchema } from "../src/deployments/release";
import { type ProjectTemplates } from "./project-templates";
import type { AppEvent, ReadRecord } from "./events";
import { localResetSummary, readReplyFile, readReplyInput, resolveScenarioPath, runScenario, ScenarioRefusal, testCalls, testControls, testReply, testSchedule } from "./local-test";
import { PULL_DIRECTORY, describePulled, downloadPackage, extractVerifiedPulledData, pullLocal, readPulled, type Pulled } from "./pull";
import { pushData } from "./push";
import {
  ACTION_CONTRACT_FILE, ACTION_CONTRACT_MARKDOWN, ACTION_CONTRACT_SCHEMA_FILE,
  actionContractSchema, generateActionContract, renderActionContractMarkdown, serializeActionContract,
  type ActionContract,
} from "../src/lib/action-contract";
import { checkHostname, type DomainRules } from "../src/lib/domain-name";
import { appEntry, assembleStaticArtifact, entryNeverRunsMessage } from "../src/lib/static-artifact.js";
import { sdkRequired, sourcesUseCalls, sourcesUseHttp } from "../src/lib/module-use";
import { sourceFilesFrom, textFiles, type SourceFiles } from "../src/lib/source-files";
import { sourceDigest } from "../src/deployments/source-digest";
import { checkSourceFiles, skippedName } from "../src/lib/source-rules";
import { checkArtifact, checkRelease } from "../src/lib/size";
import { ACTION_TYPES_FILE, renderActionTypes } from "../src/lib/action-types";
import { defaultModuleStates } from "../src/lib/app-folder";
import { bundleModule, bundleFunctions } from "../src/lib/local-builder.js";
import { parseFunctions, validateFunctions, validateServerArtifact, validateFunctionSources, serverFolderEntries, verifyServerFolder } from "../src/kernel/functions";
import { readTar } from "../src/lib/tar";
import { readAppFolder } from "../src/lib/import-folder";
import { buildProjectEntries } from "../src/lib/build-project";
import { buildProjectEntriesFromDisk } from "../src/lib/build-project-disk";
import { settingsDigest } from "./settings-digest";
import { matchesSourceRevisionScope, sourceRevisionScope } from "./source-revision";
import { requireTestKernel } from "./test-kernel";
import { importWithLocalKernel, rehearsePulledSnapshot } from "./rehearse";

const candidateRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const toolRoot = existsSync(path.join(candidateRoot, "kernel-rs/Cargo.toml")) ? candidateRoot : undefined;
type EmbeddedResources = { templates: string; runtime: string; staticArtifact: string; localBuilder: string; nodeCompat: string; nodeCompatProviders: string; parserSource: string; parserLicense: string; typeScriptLibs: Record<string, string> };
const embeddedResources = (globalThis as typeof globalThis & { __KONAE_RESOURCES__?: EmbeddedResources }).__KONAE_RESOURCES__;
let shutdownSignalCode: number | undefined;
type Project = { id: string; name: string; slug: string; manifest: Record<string, unknown>; files?: Record<string, string> };


type Config = { url?: string; project: Project; contactEmail?: string; modules?: AppSettings["modules"] };
type SignupMode = "open" | "invitation" | "closed" | "allowlist";
export type SignupPolicy = { mode: SignupMode; allowlist: string[] };

const SIGNUP_MODES: readonly SignupMode[] = ["open", "invitation", "closed", "allowlist"];
const MODE_LABELS: Record<SignupMode, string> = { open: "Open", invitation: "Invitation only", closed: "Closed", allowlist: "Allowlist" };

export const INIT_SIGNUP_POLICY = { mode: "invitation" } as const;


export function manifestSignupPolicy(manifest: Record<string, unknown>): SignupPolicy | undefined {
  const value = manifest.signupPolicy as { mode?: unknown; allowlist?: unknown } | undefined;
  if (value === undefined) return undefined;
  if (!value || typeof value !== "object" || !SIGNUP_MODES.includes(value.mode as SignupMode)) {
    throw Error('konae.json signupPolicy.mode must be "open", "invitation", "closed" or "allowlist".');
  }
  const allowlist = value.allowlist ?? [];
  if (!Array.isArray(allowlist) || !allowlist.every((entry) => typeof entry === "string")) {
    throw Error("konae.json signupPolicy.allowlist must be a list of emails and @domains.");
  }
  return { mode: value.mode as SignupMode, allowlist };
}

function describePolicy(policy: SignupPolicy) {
  return policy.mode === "allowlist" ? `Allowlist (${policy.allowlist.join(", ") || "empty"})` : MODE_LABELS[policy.mode];
}

/**
 * The warning a republish prints when konae.json names another policy
 * than the Deployment has. A publish never changes who may join an app that
 * exists (docs/reference/accounts.md, "Signup policy"), so the file and the app can differ.
 */
export function signupPolicyWarning(wanted: SignupPolicy, current: SignupPolicy): string | undefined {
  const entries = (policy: SignupPolicy) => policy.allowlist.map((entry) => entry.trim().toLowerCase()).filter(Boolean).sort().join("\n");
  if (wanted.mode === current.mode && (wanted.mode !== "allowlist" || entries(wanted) === entries(current))) return undefined;
  return `konae.json asks for signup ${describePolicy(wanted)}, but this app has ${describePolicy(current)}. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Access > Users view, or use module_set on auth with a credential that holds manage_users.`;
}


export function loginScopes(args: string[]) {
  return ["read", "write", "publish",
    ...(args.includes("--manage-users") ? ["manage_users"] : []),
    ...(args.includes("--export-data") ? ["export_data"] : [])];
}


export const INIT_TEMPLATES = ["tasks", "notes", "members", "form", "webhook", "digest", "gallery"] as const;


const DEV_SESSION = ".konae/dev.json";


export const NO_INSTALLATION = "No installation to talk to. Pass --url <installation URL>, or run login in this folder, which saves the one it signed in to. login --browser signs in through the browser without a password.";


export const DOMAIN_NEEDS_DEPLOYMENT = "Name the Deployment with --deployment <id>, or with --dir <folder> whose konae.json carries its id. project_inspect shows the ID.";


export const SERVER_COMMANDS = ["owner-code-create", "owner-code-list", "owner-code-revoke", "owner-account-suspend", "owner-account-resume", "restore", "reconcile", "files-migrate", "files-reconcile"];


export function serverCommandMessage(command: string) {
  return `${command} is a command of the konae server binary, not of this CLI. Run KONAE_DATA_DIR=data/kernel-local kernel-rs/target/debug/konae ${command} for bun run dev:local, or docker compose exec --user konae kernel konae ${command} under Compose.`;
}

/** What `konae help` prints: every command with its options, in the order a project meets them. */
export const HELP = `Usage: konae <command> [options]

login     --dir <folder> --url <installation URL> [--browser [--no-open]] [--manage-users] [--export-data]
          [--verify-from <mailbox.jsonl>]
          --browser prints an address and opens it, unless --no-open. Open it in a browser on this
          machine, sign in as the owner and allow access within 5 minutes. No password reaches the
          terminal. It saves the OAuth token returned by the installation, usually valid for 24 hours
          and capped at one hour when the installation uses Clerk.
          Without --browser, reads KONAE_EMAIL and KONAE_PASSWORD, and asks for either one that is unset
          when there is a terminal. KONAE_INVITATION_CODE creates the account with an operator's
          invitation code, and KONAE_SETUP_KEY creates the first owner.
          Saves a 24-hour token with read, write and publish in <folder>/.konae/credentials.json.
          KONAE_URL sets the default installation URL for login; otherwise it uses http://localhost:4173.
          Each flag adds that permission: inviting app users and exporting live data.
          On a Kernel that verifies email, creating the account sends a link; --verify-from reads it from
          a development mailbox on this machine and sets KONAE_PASSWORD, for loopback Kernels only.
init      --dir <empty folder> [--name <name>] [--template <id>]
          Writes konae.json, app/, compose.yaml, .env.example, .gitignore, AGENTS.md, action-contract.md,
          actions.d.ts, package.json, tsconfig.json and scripts/build.mjs, scripts/static-artifact.mjs,
          scripts/local-builder.mjs and scripts/runtime.js. Build writes dist/. It needs no
          credential and no Kernel, and the first publish registers the project.
          Templates: ${INIT_TEMPLATES.join(", ")}.
dev       --dir <folder> [--port 8796] [--events-port 8797] [--reset] [--confirm-schema <confirmation>]
          Builds dist/ with the shared local builder and runs the matching kernel binary on the folder,
          with data/ as its data. From a checkout it uses that Kernel and builds it with Cargo if absent.
          The compiled CLI downloads its matching Kernel and verifies SHA256SUMS, unless
          KONAE_KERNEL_BIN names a local binary. The Kernel checks the folder as a release. Its email,
          notices and invitations included, goes to <folder>/.konae/kernel/mail-testbox.jsonl.
          An app whose Signup Policy is not open and that holds no account gets one invited at
          test@example.com, and dev prints the link that opens it.
          A file over 2 MiB is refused as on a Kernel; KONAE_FILE_MAX_BYTES raises that limit.
          The shared local builder runs npm run build when package.json has a build script and
          node_modules exists; otherwise it uses its built-in builder. Install imported packages first.
          It sets and prints a local value for each signing secret actions.json names, and serves a
          page of what the app did (email, calls, scheduled runs, Signals) on the next port.
          --reset imports the data pull exported into data/, after a backup, or empties data/ when there is none.
          A schema change that loses data is refused; --confirm-schema applies the plan it names.
          Writes actions.d.ts, which types miniapp.actions.call.
sql       --dir <folder> [--prod] [--url <installation URL>] ("<statement>" | --file <file.sql>)
          Runs SQL on data/database.sqlite with the kernel binary's sql command. Only --prod reaches the
          published Deployment, through database_execute.
events    --dir <folder> [--follow] [--json] [--days 7] [--prod]
          Lists what the app did besides answer its page: email, calls to other services, scheduled
          runs, routes and Signals, oldest first. Locally an email shows its recipient and text,
          because nothing is sent. --json prints one object per line and --follow keeps printing.
          Only --prod reads the published Deployment.
test      --dir <folder> status|calls|reply <call-id>|schedule <action>|scenario <file>
          Reads local test state, supplies a JSON reply from --stdin or --file, runs a paused schedule,
          or consumes ordered response steps from a version 1 file under tests/. It never targets production.
pull      --dir <folder> [--url <installation URL>]
          Downloads app/, dist/, konae.json settings and data into <folder>/.konae/pull. Needs login
          --export-data. Use --data-only to refresh data without changing sources or their base markers.
          It replaces local sources and settings only when their saved digests match; otherwise it
          stages published files and preserves local edits. dev --reset imports pulled data after a backup.
          A pending pull blocks push until that import finishes.
schema    fingerprint --dir <folder> [--data] [--json]
          Prints the fingerprint of app/schema.sql, or data/database.sqlite with --data.
          Reads structure only and does not start the app or change its data.
rehearse  --dir <folder> [--confirm-schema <confirmation>]
          Runs the Kernel's data import with the current app/, settings and build against the data
          snapshot in .konae/pull, in a scratch folder. It reports candidate and baseline digests without changing data/.
push      --dir <folder> [--url <installation URL>] [--yes] [--confirm-schema <confirmation>]
          Replaces published data with <folder>/data: database, accounts, files and Usage. Import a
          pending pull with dev --reset first.
          Rows are never merged. Without --yes it prints rows here against published rows and sends
          nothing. With it, the app goes into maintenance, the import backs up the published data
          first and prints the backup's id, and maintenance ends, on failure too.
build     --dir <folder>                 Builds app/ into dist/ with the shared builder.
          It runs npm run build with a package.json build script and node_modules; otherwise it uses
          the built-in builder. Install imported npm packages first.
contract  --dir <folder> [--out <folder>]   Writes action-contract.json, its schema, action-contract.md and
          actions.d.ts, which types miniapp.actions.call.
publish   --dir <folder> [--url <installation URL>] [--plan [--json]] [--confirm-schema <confirmation>]
          Builds and publishes to --url or the installation login saved. Set each signing secret
          with secret set first. The Kernel changes the database to what schema.sql states. A
          change that drops a table or a column, or changes a column's type, prints its plan and
          asks; without a terminal, pass the plan's confirmation. --plan prepares a remote plan
          without applying it or saving publish markers; --json prints the plan as JSON.
conform   --url <app URL> --contract <file> [--token <app session>] [--key <API key>]
          [--secret <name>=<value>] [--inputs <file>]
          Calls every action and checks the answers against the contract. It runs each mutation once,
          so point it at a test Deployment.
export    --dir <folder> --out <empty folder> [--with-data]
          Writes konae.json, app/, dist/, server/ when present and generated developer files.
          Published folders retain their built version even when editable sources are ahead.
          --with-data adds the published app's data/ and needs a credential from
          login --export-data. Secrets never go with it.
domain    [--deployment <id>] [--set <hostname> | --clear] [--dir <folder>] [--url <installation URL>]
          Shows, sets or clears the app's custom domain. --deployment defaults to the id in
          <folder>/konae.json, which is the Deployment's ID. --set prints the TXT record that proves
          you control the hostname; run it again once the record is at your DNS provider.
hostname  [--deployment <id>] [--set <name>] [--dir <folder>] [--url <installation URL>]
          Shows the generated address Konae gave the app, or moves it to <name>.<app suffix>. The old
          name redirects to the new one and stays the app's. --deployment defaults as for domain.
secret    set <name> --dir <folder> [--use route|call|google] [--url <installation URL>]
          Sets one of the project's secrets. It reads the value from stdin, or asks for it without
          showing it on a terminal, so the value never sits in argv or shell history. Then it reads
          the name back with secrets_list. google_client_id and google_client_secret take use google.
          secret list --dir <folder> lists each secret's name and use, never its value.
tool      <name> ['<json arguments>'] [--stdin <key>] [--dir <folder>] [--url <installation URL>]
          Calls one tool of the owner's MCP and prints its answer as JSON. The arguments are one JSON
          object and default to {}. --stdin <key> reads that argument from standard input, so a
          secret's value stays off the command line. A refusal prints the tool's own words.

init, build, contract and dev need no credential, nor do sql and events without --prod or export without --with-data.
conform takes the app's own credentials as flags. Every other command but login reads it from
<folder>/.konae/credentials.json or KONAE_MCP_TOKEN, and talks to --url or the installation login saved in that file.
The installation URL is the editor's origin: http://localhost:4173 under bun run dev:local,
http://app.localhost:8080 under Compose.`;

function endpoint(url: string) { return new URL("/mcp", url).href; }
async function json<T>(filename: string): Promise<T> { return JSON.parse(await readFile(filename, "utf8")); }


export const VERIFY_BY_LINK = "This installation verifies email, so the account waits for the link sent to your address. Open it, choose KONAE_PASSWORD as the password, then run login again with the same KONAE_EMAIL and KONAE_PASSWORD and without the code or key, which are used now. On a Kernel on this machine, run login with --verify-from <its data folder>/mailbox.jsonl to do both in one step.";


export function isLoopbackUrl(url: string) {
  const host = new URL(url).hostname.replace(/^\[|\]$/g, "");
  return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host);
}


export function creatorVerificationToken(mailbox: string, email: string, seen = 0): string | undefined {
  const lines = mailbox.split("\n").filter(Boolean).slice(seen);
  const message = lines.map((line) => { try { return JSON.parse(line) as { to?: string; scope?: string; purpose?: string; link?: string }; } catch { return {}; } })
    .filter((item) => item.scope === "creator" && item.purpose === "verification" && item.to?.toLowerCase() === email.toLowerCase() && item.link)
    .at(-1);
  return message ? new URLSearchParams(new URL(message.link!).hash.slice(1)).get("verification") ?? undefined : undefined;
}

async function mailboxLines(file: string) {
  return (await readFile(file, "utf8").catch(() => "")).split("\n").filter(Boolean).length;
}


export async function waitForMailbox<T>(file: string, pick: (mailbox: string) => T | undefined, seconds = 15): Promise<T | undefined> {
  for (const deadline = Date.now() + seconds * 1000; Date.now() < deadline; await Bun.sleep(200)) {
    const found = pick(await readFile(file, "utf8").catch(() => ""));
    if (found !== undefined) return found;
  }
}


async function verifyFromMailbox(url: string, file: string, email: string, password: string, seen: number) {
  const token = await waitForMailbox(file, (mailbox) => creatorVerificationToken(mailbox, email, seen));
  if (token) return fetch(new URL("/_konae/owner/verify", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token, password }),
  });
  throw Error(`No verification link for ${email} reached ${file} within 15 seconds. Check that this is the mailbox of the Kernel at ${url}, which writes one when KONAE_MAIL_TRANSPORT=development.`);
}

/** The address `dev` invites to try an app whose Signup Policy is not open. */
export const TEST_ACCOUNT_EMAIL = "test@example.com";

/**
 * The newest invitation link for `email` among the development mailbox's
 * lines past the first `seen`. Lines for owners, other addresses and other
 * purposes are skipped.
 */
export function invitationLink(mailbox: string, email: string, seen = 0): string | undefined {
  return mailbox.split("\n").filter(Boolean).slice(seen)
    .map((line) => { try { return JSON.parse(line) as { to?: string; scope?: string; purpose?: string; link?: string }; } catch { return {}; } })
    .filter((item) => item.purpose === "invitation" && item.scope !== "creator" && item.to?.toLowerCase() === email.toLowerCase() && item.link)
    .at(-1)?.link;
}

/** Whether the local app holds an account, invited or active, as `kernel users` lists them. */
export async function holdsAppAccounts(kernel: Pick<LocalKernel, "run">) {
  const listed = JSON.parse(await kernel.run(["users"])) as { users?: unknown[] };
  return (listed.users?.length ?? 0) > 0;
}

/**
 * Invites the test account when the local app holds no account yet, waits for
 * the link to reach the mailbox, and returns the line that says where to open
 * it. It returns nothing when accounts exist, and the reason when the
 * invitation or its link did not come.
 */
export async function inviteTestAccount(kernel: Pick<LocalKernel, "run">, mailbox: string, seconds = 15): Promise<{ line?: string; failed?: string }> {
  if (await holdsAppAccounts(kernel)) return {};
  const seen = await mailboxLines(mailbox);
  try { await kernel.run(["invite", TEST_ACCOUNT_EMAIL]); } catch (error) { return { failed: error instanceof Error ? error.message : String(error) }; }
  const link = await waitForMailbox(mailbox, (text) => invitationLink(text, TEST_ACCOUNT_EMAIL, seen), seconds);
  return link
    ? { line: `Test account: open ${link} and choose a password.` }
    : { failed: `The invitation for ${TEST_ACCOUNT_EMAIL} did not reach ${mailbox} within ${seconds} seconds.` };
}

/** Signs in, or creates the account with an invitation code or, for the first owner, the setup key. */
async function login(url: string, email: string, password: string, admission: { code?: string; setupKey?: string } = {}, scopes = ["read", "write", "publish"], verifyFrom?: string): Promise<string> {
  if (verifyFrom && !isLoopbackUrl(url)) throw Error(`--verify-from reads a development mailbox on this machine, so it works only with a Kernel on localhost or 127.0.0.1, not ${new URL(url).host}.`);
  let response = await fetch(new URL("/_konae/owner/login", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password }),
  });
  const seen = verifyFrom ? await mailboxLines(verifyFrom) : 0;
  if (!response.ok && (admission.code || admission.setupKey)) response = await fetch(new URL("/_konae/owner/signup", url), {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password, ...admission }),
  });
  if (response.status === 202 && verifyFrom) response = await verifyFromMailbox(url, verifyFrom, email, password, seen);
  if (response.status === 202) throw Error(VERIFY_BY_LINK);
  if (!response.ok) throw Error((await response.json()).error ?? "Owner sign-in failed.");
  const cookie = response.headers.get("set-cookie")?.split(";")[0];
  if (!cookie) throw Error("Owner sign-in returned no session.");
  const client = new McpClient(endpoint(url), { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const token = await client.call<{ token: string }>("token_create", { scopes, ttlSeconds: 86400 });
  await fetch(new URL("/_konae/owner/logout", url), { method: "POST", headers: { cookie } });
  return token.token;
}

/** How long `login --browser` waits for the owner to finish in the browser. */
export const BROWSER_LOGIN_TIMEOUT_MS = 5 * 60_000;

function base64url(bytes: Uint8Array) {
  return Buffer.from(bytes).toString("base64url");
}

/** Opens `url` in the system browser. Failing to is fine: the URL is printed too. */
function openBrowser(url: string) {
  const command = process.platform === "darwin" ? ["open", url] : process.platform === "win32" ? ["cmd", "/c", "start", "", url] : ["xdg-open", url];
  try { Bun.spawn(command, { stdin: "ignore", stdout: "ignore", stderr: "ignore" }).exited.catch(() => {}); } catch { /* printed below */ }
}

/**
 * Signs the owner in through the browser: the installation's OAuth code flow
 * with PKCE, redirected to a port on 127.0.0.1 that this process listens on
 * (RFC 8252). A loopback client's token lasts as long as one from password
 * login. The code and the token never reach the terminal.
 */
export async function browserLogin(url: string, scopes: string[], options: { open?: boolean; timeoutMs?: number; print?: (line: string) => void; openUrl?: (url: string) => void } = {}): Promise<{ token: string; expiresIn: number }> {
  const print = options.print ?? console.log;
  const timeoutMs = options.timeoutMs ?? BROWSER_LOGIN_TIMEOUT_MS;
  const discovered = await fetch(new URL("/.well-known/oauth-protected-resource", url)).catch(() => undefined);
  if (!discovered?.ok) throw Error(`${url} offers no owner sign-in (${discovered ? `/.well-known/oauth-protected-resource answered ${discovered.status}` : "it did not answer"}). Pass --url with the address that serves the editor, not an app's.`);
  const { resource, authorization_servers: servers } = await discovered.json() as { resource: string; authorization_servers: string[] };
  const metadata = await fetch(new URL("/.well-known/oauth-authorization-server", servers?.[0] ?? url)).then((response) => response.json()) as { issuer: string; authorization_endpoint: string; token_endpoint: string; registration_endpoint: string };

  const verifier = base64url(crypto.getRandomValues(new Uint8Array(32)));
  const challenge = base64url(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));
  const state = base64url(crypto.getRandomValues(new Uint8Array(24)));
  let settle!: { resolve(code: string): void; reject(error: Error): void };
  const answered = new Promise<string>((resolve, reject) => { settle = { resolve, reject }; });
  const page = (status: number, text: string) => new Response(`<!doctype html><meta charset=utf-8><title>konae login</title><p>${text}</p>`, { status, headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" } });
  const listener = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch(request) {
      const callback = new URL(request.url);
      if (callback.pathname !== "/callback") return page(404, "Nothing here.");
      // A request without this login's state is a stray one, so the login keeps waiting.
      if (callback.searchParams.get("state") !== state) return page(400, "This sign-in does not belong to the konae login waiting in the terminal. Start the sign-in from the address that login printed.");
      const iss = callback.searchParams.get("iss");
      if (iss && iss !== metadata.issuer) return page(400, `This answer came from ${iss}, not ${metadata.issuer}, so login ignored it.`);
      const error = callback.searchParams.get("error");
      const code = callback.searchParams.get("code");
      if (error || !code) {
        settle.reject(Error(`The installation refused the sign-in: ${callback.searchParams.get("error_description") ?? error ?? "it sent no code"}. Run login --browser again.`));
        return page(400, "The sign-in was refused. The terminal says why.");
      }
      settle.resolve(code);
      return page(200, "Signed in. Close this tab and go back to the terminal.");
    },
  });
  const redirectUri = `http://127.0.0.1:${listener.port}/callback`;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const registered = await fetch(metadata.registration_endpoint, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ client_name: "konae CLI", redirect_uris: [redirectUri], token_endpoint_auth_method: "none" }),
    });
    if (registered.status === 429) throw Error(`This address registered too many sign-in clients in the last hour, so the installation refused another. Try login --browser again in ${Math.ceil(Number(registered.headers.get("retry-after") ?? 3600) / 60)} minutes.`);
    const client = await registered.json() as { client_id?: string; error_description?: string };
    if (!registered.ok || !client.client_id) throw Error(`The installation refused to register this login: ${client.error_description ?? registered.status}.`);
    const authorize = new URL(metadata.authorization_endpoint);
    for (const [key, value] of Object.entries({
      response_type: "code", client_id: client.client_id, redirect_uri: redirectUri, scope: scopes.join(" "),
      state, code_challenge: challenge, code_challenge_method: "S256", resource,
    })) authorize.searchParams.set(key, value);
    print(`Open this address in a browser on this machine, sign in as the owner and allow access:\n\n  ${authorize.href}\n\nWaiting up to ${Math.round(timeoutMs / 60_000)} minutes.`);
    if (options.open !== false) (options.openUrl ?? openBrowser)(authorize.href);
    const timeout = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(Error(`Nobody finished the sign-in within ${Math.round(timeoutMs / 60_000)} minutes, so login saved nothing. Run login --browser again.`)), timeoutMs); });
    const code = await Promise.race([answered, timeout]);
    const exchanged = await fetch(metadata.token_endpoint, {
      method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ grant_type: "authorization_code", code, client_id: client.client_id, redirect_uri: redirectUri, resource, code_verifier: verifier }),
    });
    const token = await exchanged.json() as { access_token?: string; expires_in?: number; error_description?: string };
    if (!exchanged.ok || !token.access_token) throw Error(`The installation refused to issue the token: ${token.error_description ?? exchanged.status}. Run login --browser again.`);
    return { token: token.access_token, expiresIn: token.expires_in ?? 3600 };
  } finally {
    clearTimeout(timer);
    listener.stop(true);
  }
}

/** The uses secret_set takes. A Google client half defaults to google, any other name to route. */
export const SECRET_USES = ["route", "call", "google"] as const;

/** A secret's value from piped stdin, without the one line break `echo` or a file ends with. */
export function pipedSecret(text: string) {
  return text.replace(/\r?\n$/, "");
}

/**
 * Sets a project's secret through secret_set, then reads the names back
 * through secrets_list. The value is never printed and never returned.
 */
export async function setSecret(client: Pick<McpClient, "call">, projectId: string, name: string, value: string, use?: string) {
  await client.call("secret_set", { projectId, name, value, ...(use ? { use } : {}) });
  const listed = await client.call<{ secrets: { name: string; use: string; updatedAt: number }[] }>("secrets_list", { projectId });
  const secret = listed.secrets.find((item) => item.name === name);
  if (!secret) throw Error(`secret_set answered, but secrets_list does not list ${name}. Run secret list to see what the project holds.`);
  return `Set ${name} on ${projectId} for use ${secret.use} at ${new Date(secret.updatedAt).toISOString()}. secrets_list lists it and never returns its value.`;
}

/** "24-hour" or "1-hour", from a token's lifetime in seconds. */
function lifetimeLabel(seconds: number) {
  return seconds % 3600 === 0 ? `${seconds / 3600}-hour` : `${Math.round(seconds / 60)}-minute`;
}

async function credentials(root: string) {
  if (process.env.KONAE_MCP_TOKEN) return process.env.KONAE_MCP_TOKEN;
  return (await json<{ token: string }>(path.join(root, ".konae/credentials.json"))).token;
}

/** The folder's sources: `app/`. */
export function appDir(root: string) { return path.join(root, SOURCES_FOLDER); }

/** Every file of `app/`, which a release carries as the project's files, as text or as base64 that `encodings` names. */
async function sources(root: string): Promise<SourceFiles> {
  const paths = await capsulePaths(root);
  return sourceFilesFrom(await Promise.all(paths.map(async (relative) => [relative, new Uint8Array(await readFile(path.join(appDir(root), relative)))] as const)));
}

/** Every file of `app/` the build may publish, text or binary. Dot folders and node_modules are skipped at any depth. */
async function capsulePaths(root: string, prefix = ""): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(path.join(appDir(root), prefix), { withFileTypes: true })) {
    if (skippedName(entry.name)) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await capsulePaths(root, relative));
    else if (entry.isFile()) result.push(relative);
  }
  return result;
}

async function appDirectories(root: string, prefix = ""): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(path.join(appDir(root), prefix), { withFileTypes: true })) {
    if (skippedName(entry.name) || !entry.isDirectory()) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    result.push(relative, ...await appDirectories(root, relative));
  }
  return result;
}

/** Reads konae.json. */
export async function readSettings(root: string): Promise<AppSettings> {
  try { return await json<AppSettings>(path.join(root, SETTINGS_FILE)); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") throw Error(`No konae.json in ${root}. Run init there, or name the app folder with --dir.`);
    throw error;
  }
}

/** The project a release names, from konae.json. The Signup Policy rides in the manifest until the release reads it. */
export function configOf(settings: AppSettings, url?: string): Config {
  return {
    ...(url ? { url } : {}),
    ...(settings.contactEmail ? { contactEmail: settings.contactEmail } : {}),
    modules: defaultModuleStates(settings.modules),
    project: { id: settings.id, name: settings.name, slug: settings.slug, manifest: { ...settings.manifest, visibility: settings.visibility, signupPolicy: settings.signupPolicy } },
  };
}

/** A file as text when it is UTF-8, else as bytes, which the artifact carries as base64. */
async function readCapsuleFile(root: string, relative: string): Promise<string | Uint8Array> {
  const bytes = new Uint8Array(await readFile(path.join(appDir(root), relative)));
  try { return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes); } catch { return bytes; }
}

/** True when the app folder's package.json names a build script that writes dist/ and its packages are installed. */
async function hasPackageBuild(root: string) {
  try {
    const manifest = JSON.parse(await readFile(path.join(root, "package.json"), "utf8")) as { scripts?: Record<string, string> };
    if (typeof manifest.scripts?.build !== "string") return false;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  return (await stat(path.join(root, "node_modules")).catch(() => undefined))?.isDirectory() ?? false;
}

async function refreshPageBuilder(root: string, sdk: boolean, authored: ReadonlySet<string> = new Set()) {
  const generated = await folderFiles(root, await readSettings(root));
  const resources = generated.filter((entry) => entry.path === "scripts/build.mjs" || entry.path === "scripts/local-builder.mjs"
    || entry.path === "scripts/node-compat.js" || entry.path === "scripts/node-compat-providers.js"
    || entry.path === "scripts/static-artifact.mjs" || entry.path === "scripts/babel-parser.cjs" || entry.path === "scripts/BABEL-PARSER-LICENSE"
    || sdk && entry.path === "scripts/runtime.js").map(({ path: relative, contents }) => [relative, contents] as const);
  for (const [relative, contents] of resources) {
    if (authored.has(relative)) continue;
    const target = path.join(root, relative);
    if (await readFile(target, "utf8").catch(() => undefined) === contents) continue;
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, contents);
  }
}

/** Packs an existing dist/ into the artifact shape a release uploads. */
async function artifactFromDist(root: string, prefix = ""): Promise<{ files: Record<string, string>; encodings: Record<string, string> }> {
  const files: Record<string, string> = {};
  const encodings: Record<string, string> = {};
  const dist = path.join(root, DIST_FOLDER);
  for (const entry of await readdir(path.join(dist, prefix), { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      const nested = await artifactFromDist(root, relative);
      Object.assign(files, nested.files);
      Object.assign(encodings, nested.encodings);
      continue;
    }
    const bytes = new Uint8Array(await readFile(path.join(dist, relative)));
    try {
      files[relative] = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
    } catch {
      files[relative] = Buffer.from(bytes).toString("base64");
      encodings[relative] = "base64";
    }
  }
  return { files, encodings };
}

/**
 * Runs an npm command in the app folder and shows its output only when it
 * fails, so a command's answer, such as the published URL, is all it prints.
 */
async function quietly(command: string[], cwd: string) {
  const child = Bun.spawn(command, { cwd, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (code !== 0) process.stderr.write(stdout + stderr);
  return code === 0;
}

/**
 * Builds `app/` into `dist/` by the static publishing contract in
 * src/lib/static-artifact.js, the rules the editor's build runs too.
 * When the folder has package.json with a build script (from init) and a
 * node_modules folder, that script writes dist/ and this packs it. Otherwise
 * the built-in bundler runs, so a folder nobody ran npm install in builds too.
 * `development` asks that script for an unminified build, which `dev` serves.
 * Answers the artifact, `source`, every file of `app/` as a release carries
 * it, and `files`, the text files of `app/` by path.
 */
export async function buildLocal(root: string, options: { development?: boolean; customBuild?: boolean } = {}) {
  const source = await sources(root);
  const files = textFiles(source);
  const buildProject = buildProjectEntries(source);
  await writeEntries(root, buildProject);
  const authoredBuildFiles = new Set(buildProject.map((entry) => entry.path));
  const settings = await readSettings(root).catch(() => undefined);
  const functionsSource = files["functions.json"];
  const functions = functionsSource === undefined ? undefined : parseFunctions(functionsSource);
  const functionsContext = { files: Object.keys(files), modules: settings?.modules, actions: JSON.parse(files["actions.json"] ?? '{"actions":{}}').actions ?? {} };
  if (functions) {
    validateFunctionSources(functions, files);
    validateFunctions(functions, functionsContext);
  }
  const serverArtifact = functions && Object.keys(functions.functions).length ? await bundleFunctions(appDir(root), functions, options.development) : undefined;
  validateServerArtifact(functions, serverArtifact);
  const saveServer = async (staticPaths: string[]) => {
    if (functions) validateFunctions(functions, { ...functionsContext, staticPaths });
    await rm(path.join(root, "server"), { recursive: true, force: true });
    if (serverArtifact) await writeEntries(root, await serverFolderEntries(serverArtifact));
  };
  const sdkEnabled = sdkRequired(settings?.modules, files, settings?.visibility ?? "private");
  await refreshActionTypes(root);
  const paths = await capsulePaths(root);
  if (paths.some((file) => file.startsWith("src/") && /\.(?:ts|tsx|mts|cts)$/.test(file) && !/\.d\.[mc]?ts$/.test(file))) typecheckApp(root);
  if (await hasPackageBuild(root)) {
    await refreshPageBuilder(root, sdkEnabled, authoredBuildFiles);
    // A folder's own scripts/static-artifact.mjs may predate this refusal, so it runs here too.
    const neverRuns = entryNeverRunsMessage(Object.keys(source.files), files, sdkEnabled);
    if (neverRuns) throw Error(neverRuns);
    // The script type-checks against actions.d.ts, so it reads the actions.json of this build.
    if (options.customBuild === false) {
      if (!existsSync(path.join(root, DIST_FOLDER, "index.html"))) throw Error("The app has no dist/index.html. Run `konae build` to run its local build before publishing.");
    } else if (!(await quietly(["npm", "run", "build", ...(options.development ? ["--", "--dev"] : [])], root))) throw Error("npm run build failed.");
    const packed = await artifactFromDist(root);
    if (!packed.files["index.html"]) throw Error("npm run build wrote no dist/index.html.");
    const artifact = { version: 1 as const, entrypoint: "index.html", files: packed.files, encodings: Object.keys(packed.encodings).length ? packed.encodings : undefined };
    await saveServer(Object.keys(artifact.files));
    return { artifact, serverArtifact, source, files };
  }
  const sdkRuntime = embeddedResources?.runtime ?? await readFile(path.join(toolRoot!, "src/mcp/runtime.js"), "utf8");
  const entry = appEntry(Object.keys(files)) ?? "app.js";
  const artifact = await assembleStaticArtifact({
    paths,
    read: (relative) => readCapsuleFile(root, relative),
    compile: async () => {
      const { script, styles } = await bundleModule(appDir(root), entry, options.development);
      return { script, styles: [files["styles.css"] ?? "", styles].filter(Boolean).join("\n") };
    },
    bundle: (relative) => bundleModule(appDir(root), relative, options.development),
    runtime: sdkRuntime,
    sdk: sdkEnabled,
    routing: settings?.manifest?.routing as "pages" | "spa" | undefined,
    warn: (message) => console.warn(`Warning: ${message}`),
  });
  await saveServer(Object.keys(artifact.files));
  await writeDist(root, artifact);
  return { artifact, serverArtifact, source, files };
}

/** Checks app TypeScript with the same tsconfig used by the folder build script. */
function typecheckApp(root: string) {
  if (!existsSync(path.join(root, "tsconfig.json"))) return;
  const configFile = ts.readConfigFile(path.join(root, "tsconfig.json"), ts.sys.readFile);
  if (configFile.error) throw Error(ts.flattenDiagnosticMessageText(configFile.error.messageText, "\n"));
  const config = ts.parseJsonConfigFileContent(configFile.config, ts.sys, root);
  const host = ts.createCompilerHost(config.options);
  const libDirectory = path.dirname(ts.getDefaultLibFilePath(config.options));
  const embeddedLibrary = (fileName: string) => {
    if (path.resolve(path.dirname(fileName)) !== path.resolve(libDirectory)) return undefined;
    return embeddedResources?.typeScriptLibs[path.basename(fileName)];
  };
  const originalFileExists = host.fileExists.bind(host);
  const originalReadFile = host.readFile.bind(host);
  const originalGetSourceFile = host.getSourceFile.bind(host);
  host.fileExists = (fileName) => embeddedLibrary(fileName) !== undefined || originalFileExists(fileName);
  host.readFile = (fileName) => embeddedLibrary(fileName) ?? originalReadFile(fileName);
  host.getSourceFile = (fileName, languageVersion, onError, shouldCreateNewSourceFile) => {
    const library = embeddedLibrary(fileName);
    return library === undefined
      ? originalGetSourceFile(fileName, languageVersion, onError, shouldCreateNewSourceFile)
      : ts.createSourceFile(fileName, library, languageVersion);
  };
  const diagnostics = [...config.errors, ...ts.getPreEmitDiagnostics(ts.createProgram(config.fileNames, config.options, host))];
  if (diagnostics.length) {
    const formatted = ts.formatDiagnosticsWithColorAndContext(diagnostics, {
      getCurrentDirectory: ts.sys.getCurrentDirectory,
      getCanonicalFileName: (file) => file,
      getNewLine: () => ts.sys.newLine,
    });
    throw Error(`The type check failed, so dist/ is unchanged. Fix the errors above. A call to miniapp.actions.call must match an action in actions.d.ts, which konae dev and konae contract write from app/actions.json.\n${formatted}`);
  }
}

/**
 * Builds and publishes. On a first release `project.manifest.signupPolicy`
 * goes as `initialSignupPolicy`; on a later one the Kernel keeps the app's
 * policy and `warn` gets one line when konae.json names another.
 */
async function productionRequirements(root: string, confirmation?: string | null) {
  const requirements = confirmation ? [`Pass --confirm-schema ${confirmation} when applying this database change.`] : [];
  try {
    const actions = (JSON.parse(await readFile(path.join(appDir(root), "actions.json"), "utf8")) as { actions?: Record<string, { http?: { signature?: { secret?: string } }; auth?: { secret?: string } }> }).actions ?? {};
    const secrets = new Set<string>();
    for (const action of Object.values(actions)) {
      if (action.http?.signature?.secret) secrets.add(action.http.signature.secret);
      if (action.auth?.secret) secrets.add(action.auth.secret);
    }
    for (const name of [...secrets].sort()) requirements.push(`Set signing secret ${name} on the production app before its signed route or call runs.`);
  } catch { /* An invalid Capsule already fails the existing build and release checks. */ }
  return requirements;
}

export async function publishLocal(root: string, config: Config, token: string, warn: (message: string) => void = console.warn, confirm: ConfirmSchema = keepData, planOnly = false) {
  if (!config.url) throw Error(NO_INSTALLATION);
  const initialModules = config.modules ?? defaultModuleStates();
  const client = new McpClient(endpoint(config.url), { token });
  const wanted = manifestSignupPolicy(config.project.manifest);
  // The policy is not part of the Capsule manifest, so the Kernel's project record never holds an allowlist.
  const { signupPolicy: _, runtime: _runtime, ...manifest } = config.project.manifest;
  const project = { ...config.project, manifest };
  const prepared = await buildLocal(root, { customBuild: false });
  const sourcePayload = {
    files: prepared.source.files,
    ...(prepared.source.encodings && Object.keys(prepared.source.encodings).length ? { encodings: prepared.source.encodings } : {}),
    directories: await appDirectories(root),
  };
  checkSourceFiles(sourcePayload);
  checkArtifact(prepared.artifact);
  const identity = { id: project.id, name: project.name, slug: project.slug, manifest: project.manifest };
  const initialSignupPolicy = wanted ? { mode: wanted.mode, allowlist: wanted.allowlist } : undefined;
  checkRelease(new TextEncoder().encode(JSON.stringify({ project: identity, visibility: manifest.visibility ?? "private", artifact: prepared.artifact, ...(prepared.serverArtifact ? { serverArtifact: prepared.serverArtifact } : {}), ...(initialSignupPolicy ? { initialSignupPolicy } : {}), ...(config.contactEmail ? { contactEmail: config.contactEmail } : {}), initialModules })));
  let first = false;
  let observedSourcesRevision: number | undefined;
  let publishedSourceDigest = "";
  publishedSourceDigest = await sourceDigest(prepared.source);
  if (planOnly) {
    const listed = await client.call<{ projects: { project: { id: string } }[] }>("projects_list", {});
    if (!listed.projects.some(({ project: remoteProject }) => remoteProject.id === project.id)) {
      return {
        target: { projectId: project.id, projectName: project.name, installation: config.url },
        sourceRevision: { observed: null, planned: null },
        sourceDigest: publishedSourceDigest,
        artifactDigest: createHash("sha256").update(JSON.stringify(prepared.serverArtifact ? { artifact: prepared.artifact, serverArtifact: prepared.serverArtifact } : prepared.artifact)).digest("hex"),
        plan: { mode: "local-first-release" },
        expiresAt: null,
        productionRequirements: ["This project is not registered on the installation. A normal publish will register it and create its first Deployment.", ...await productionRequirements(root)],
      };
    }
  }
  const result = await publishProject<{ id: string; url: string }>(client, project, async () => {
    // project_inspect needs the project that project_create registers on a first publish.
    const remote = await client.call<{ deployment?: { id: string } | null; sourcesRevision: number }>("project_inspect", { projectId: project.id });
    first = !remote.deployment;
    const marker = await json<{ sourcesRevision: number; installation?: string; projectId?: string }>(path.join(root, ".konae/source-revision.json")).catch(() => undefined);
    const scopedMarker = matchesSourceRevisionScope(marker, config.url!, project.id) ? marker : undefined;
    observedSourcesRevision = scopedMarker?.sourcesRevision ?? (!remote.deployment && remote.sourcesRevision === 0 ? 0 : undefined);
    const { artifact, serverArtifact, source } = prepared;
    return {
      project: { ...project, ...source }, visibility: manifest.visibility ?? "private", artifact, ...(serverArtifact ? { serverArtifact } : {}),
      initialSignupPolicy: first ? wanted : undefined, contactEmail: first ? config.contactEmail : undefined,
      initialModules: first ? initialModules : undefined,
    };
  }, confirm, (error) => warn(`Could not release the upload: ${error instanceof Error ? error.message : error}`), (plan) => {
    if (plan.firstRelease && plan.url) warn(firstReleaseAddress(plan.url, project.id));
  }, () => observedSourcesRevision, !planOnly, !planOnly);
  const { plan } = result;
  if (planOnly) {
    return {
      target: { projectId: project.id, projectName: project.name, installation: config.url },
      sourceRevision: { observed: observedSourcesRevision ?? null, planned: plan.expectedSourcesRevision },
      sourceDigest: publishedSourceDigest,
      artifactDigest: createHash("sha256").update(JSON.stringify(prepared.serverArtifact ? { artifact: prepared.artifact, serverArtifact: prepared.serverArtifact } : prepared.artifact)).digest("hex"),
      plan,
      expiresAt: new Date(plan.expiresAt).toISOString(),
      productionRequirements: await productionRequirements(root, plan.schema?.confirmation),
    };
  }
  const receipt = result.receipt!;
  await mkdir(path.join(root, ".konae"), { recursive: true });
  let publishedSettings: unknown;
  try { publishedSettings = JSON.parse(await readFile(path.join(root, SETTINGS_FILE), "utf8")); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    publishedSettings = { project: config.project, contactEmail: config.contactEmail };
  }
  await writeFile(path.join(root, ".konae/source-revision.json"), JSON.stringify({ ...sourceRevisionScope(config.url, project.id), sourcesRevision: plan.expectedSourcesRevision + 1, sourceDigest: publishedSourceDigest, settingsDigest: settingsDigest(publishedSettings) }, null, 2), { mode: 0o600 });
  await writeFile(path.join(root, ".konae/release-plan.json"), JSON.stringify(plan, null, 2));
  await writeFile(path.join(root, ".konae/published.json"), JSON.stringify(receipt, null, 2));
  if (!first && Object.keys(config.modules ?? {}).length > 0) {
    warn("konae.json modules apply only to the first publish. This release keeps the Deployment's module state. Read it with modules_read and change it with module_set.");
  }
  if (!first && wanted) {
    // Reading the policy needs only read. A Kernel that cannot answer leaves the publish as it is.
    const inspected = await client.call<{ deployment?: { signupPolicy?: { mode: SignupMode; allowlist?: string[] } } | null }>("project_inspect", { projectId: project.id }).catch(() => undefined);
    const current = inspected?.deployment?.signupPolicy;
    const warning = current?.mode && signupPolicyWarning(wanted, { mode: current.mode, allowlist: current.allowlist ?? [] });
    if (warning) warn(warning);
  }
  if (!first && config.contactEmail) {
    const current = await client.call<{ contactEmail: string | null }>("contact_email_read", { projectId: project.id }).catch((error) => {
      warn(`Publish succeeded, but contact_email_read failed: ${error instanceof Error ? error.message : error}`);
      return undefined;
    });
    if (current && current.contactEmail?.trim().toLowerCase() !== config.contactEmail.trim().toLowerCase()) {
      warn(`The app folder asks for contact email ${config.contactEmail}, but this app has ${current.contactEmail ?? "none"}. A publish keeps the app's contact email after its first release. Change it with contact_email_set.`);
    }
  }
  return Object.assign(receipt, { first });
}


export const firstReleaseAddress = (url: string, id: string) =>
  `This first release names the app ${url}, and every later release keeps that name. To use another name under the same suffix, run konae hostname --deployment ${id} --set <name> before you register Google sign-in or share links.`;

/** hostname_set, or domain_read without `set`. The Kernel checks the name, and its refusal reaches the caller unchanged. Returns the lines to print. */
export async function hostnameLocal(client: Pick<McpClient, "call">, deploymentId: string, set?: string): Promise<string[]> {
  if (set === undefined) {
    const { generatedUrl } = await client.call<{ generatedUrl: string }>("domain_read", { projectId: deploymentId });
    return [`This app answers at ${generatedUrl}.`];
  }
  const current = await client.call<{ deployment?: { hostname: string } | null }>("project_inspect", { projectId: deploymentId });
  const moved = await client.call<{ url: string; changed: boolean; previousHostnames: string[]; googleCallback: string }>("hostname_set", { projectId: deploymentId, name: set, expected: current.deployment?.hostname });
  if (!moved.changed) return [`This app already answers at ${moved.url}.`];
  return [
    `This app now answers at ${moved.url}. ${moved.previousHostnames.at(-1)} redirects there with a 308 and stays this app's.`,
    `App users sign in again on the new name. If the app offers Google sign-in, add ${moved.googleCallback} to its Google client's redirect URIs. Give the new address to any service that calls the app's HTTP routes.`,
  ];
}

/** Keeps the data, so a schema change that loses some is not applied. */
async function keepData() { return false; }

/**
 * Asks on the terminal before a schema change that loses data. The plan is
 * printed with its confirmation; `--confirm-schema <confirmation>` answers
 * yes to that plan only, and without a terminal the answer is no.
 */
export function confirmOnTerminal(accepted?: string): ConfirmSchema {
  return async (change) => {
    for (const line of describeSchemaChange(change)) console.log(line);
    if (accepted !== undefined) return accepted === change.confirmation;
    if (!process.stdin.isTTY) {
      console.log(`To apply it, run the command again with --confirm-schema ${change.confirmation}`);
      return false;
    }
    const prompt = createInterface({ input: process.stdin, output: process.stdout });
    try { return /^y(?:es)?$/i.test((await prompt.question("Apply this change? [y/N] ")).trim()); } finally { prompt.close(); }
  };
}

/** The part of a terminal input stream the password prompt uses, so a test can stand in for it. */
export type PromptInput = {
  isTTY?: boolean; setRawMode?(on: boolean): unknown; resume(): unknown; pause(): unknown; setEncoding?(encoding: string): unknown;
  on(event: "data", listener: (chunk: string) => void): unknown; off(event: "data", listener: (chunk: string) => void): unknown;
};

/**
 * Reads a line with the typed characters not echoed: the input goes into raw
 * mode, which is restored on Enter and on Ctrl-C, where the promise rejects.
 */
export function readSecret(question: string, input: PromptInput = process.stdin, output: { write(text: string): unknown } = process.stdout): Promise<string> {
  return new Promise((resolve, reject) => {
    let typed = "";
    output.write(question);
    const finish = (cancelled: boolean) => {
      input.off("data", onData);
      input.setRawMode?.(false);
      input.pause();
      output.write("\n");
      if (cancelled) reject(Error("Cancelled.")); else resolve(typed);
    };
    const onData = (chunk: string) => {
      for (const character of chunk) {
        if (character === "\r" || character === "\n") return finish(false);
        if (character === "\u0003" || character === "\u0004") return finish(true);
        if (character === "\u007f" || character === "\b") typed = typed.slice(0, -1);
        else if (character >= " ") typed += character;
      }
    };
    input.setEncoding?.("utf8");
    input.setRawMode?.(true);
    input.on("data", onData);
    input.resume();
  });
}

/** Reads one line from the terminal, echoing it unless `secret`. */
export async function askOnTerminal(question: string, secret: boolean): Promise<string> {
  if (secret) return readSecret(question);
  const prompt = createInterface({ input: process.stdin, output: process.stdout });
  try { return await prompt.question(question); } finally { prompt.close(); }
}

/**
 * The email and password `login` signs in with: the variables when set,
 * otherwise a prompt when there is a terminal, so a script without one fails
 * as before. The password is not echoed.
 */
export async function loginCredentials(env: Record<string, string | undefined>, terminal: { isTTY?: boolean }, ask: (question: string, secret: boolean) => Promise<string> = askOnTerminal) {
  let email = env.KONAE_EMAIL; let password = env.KONAE_PASSWORD;
  if (terminal.isTTY) {
    email ||= (await ask("Email: ", false)).trim();
    if (email) password ||= await ask(`Password for ${email}: `, true);
  }
  if (!email || !password) throw Error("Set KONAE_EMAIL and KONAE_PASSWORD, or run login --browser to sign in through the browser without them. To create the account, also set KONAE_INVITATION_CODE, or KONAE_SETUP_KEY for the first owner.");
  return { email, password };
}

/** Replaces `dist/` with the built artifact's files. */
async function writeDist(root: string, artifact: { files: Record<string, string>; encodings?: Record<string, string> }) {
  const dist = path.join(root, DIST_FOLDER);
  await rm(dist, { recursive: true, force: true });
  for (const [file, contents] of Object.entries(artifact.files)) {
    const target = path.join(dist, file);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, artifact.encodings?.[file] === "base64" ? Buffer.from(contents, "base64") : contents);
  }
}

/** The files beside app/, dist/ and data/ that every app folder carries. */
async function folderFiles(root: string, settings: AppSettings) {
  const read = (file: string) => readFile(path.join(appDir(root), file), "utf8").catch(() => "");
  const publishedServer = await readFile(path.join(root, "server/manifest.json"), "utf8").catch(() => "");
  let hasFunctions = false;
  if (publishedServer) {
    const manifest = JSON.parse(publishedServer) as { manifest?: { functions?: Record<string, unknown> } };
    hasFunctions = Object.keys(manifest.manifest?.functions ?? {}).length > 0;
  }
  return generatedFiles({
    settings, actionsSource: await read("actions.json") || '{"version":1,"actions":{}}', schemaSource: await read("schema.sql"),
    hasFunctions,
    runtime: embeddedResources?.runtime ?? await readFile(path.join(toolRoot!, "src/mcp/runtime.js"), "utf8"),
    staticArtifact: embeddedResources?.staticArtifact ?? await readFile(path.join(toolRoot!, "src/lib/static-artifact.js"), "utf8"),
    localBuilder: embeddedResources?.localBuilder ?? await readFile(path.join(toolRoot!, "src/lib/local-builder.js"), "utf8"),
    nodeCompat: embeddedResources?.nodeCompat ?? await readFile(path.join(toolRoot!, "src/lib/node-compat.js"), "utf8"),
    nodeCompatProviders: embeddedResources?.nodeCompatProviders ?? await readFile(path.join(toolRoot!, "src/lib/node-compat-providers.js"), "utf8"),
    parserSource: embeddedResources?.parserSource ?? await readFile(path.join(toolRoot!, "node_modules/@babel/parser/lib/index.js"), "utf8"),
    parserLicense: embeddedResources?.parserLicense ?? await readFile(path.join(toolRoot!, "node_modules/@babel/parser/LICENSE"), "utf8"),
  });
}

async function writeEntries(root: string, entries: FolderEntry[]) {
  for (const entry of entries) {
    if (entry.path === ".gitignore" && existsSync(path.join(root, entry.path))) continue;
    const target = path.resolve(root, entry.path);
    if (!target.startsWith(path.resolve(root) + path.sep)) throw Error(`Invalid folder path ${entry.path}.`);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, entry.contents);
  }
}

/**
 * Exporta fuentes y artefactos a una carpeta vacía. Una publicación conserva
 * sus bundles aunque las fuentes hayan cambiado. Con credencial export_data,
 * project_export aporta la carpeta publicada y sus datos.
 */
export async function exportLocal(root: string, output: string, remote?: { url: string; token: string }) {
  await assertEmptyFolder(output);
  const settings = await readSettings(root);
  if (remote) {
    const client = new McpClient(endpoint(remote.url), { token: remote.token });
    const published = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: settings.id });
    if (!published.deployment) throw Error("Publish the app before exporting its data, or leave out --with-data.");
    const scratch = await mkdtemp(path.join(tmpdir(), "konae-export-"));
    try {
      const downloaded = await downloadPackage(client, published.deployment.id, scratch);
      if (!downloaded.folderFile) throw Error("The export has no app folder download.");
      const archive = new Uint8Array(await readFile(downloaded.folderFile));
      const folder = readAppFolder(archive);
      const buildFiles = buildProjectEntries(folder.sources);
      await verifyServerFolder(folder.server ?? {});
      await writeEntries(output, readTar(archive));
      for (const { name } of downloaded.manifest.contents) {
        const component = downloaded.files.find((file) => file.name === name);
        if (!component) throw Error(`The export is missing ${name}.`);
        await writeEntries(output, dataEntries(name, new Uint8Array(await readFile(component.file))));
      }
      await writeEntries(output, await folderFiles(output, folder.settings));
      await writeEntries(output, buildFiles);
      return { output, data: true };
    } finally {
      await rm(scratch, { recursive: true, force: true });
    }
  }
  const hasBuildProject = (await buildProjectEntriesFromDisk(appDir(root))).length > 0;
  if (hasBuildProject && !existsSync(path.join(root, DIST_FOLDER, "index.html"))) throw Error("The app has no dist/index.html. Run `konae build` before exporting its custom build project.");
  if (!hasBuildProject && (!settings.released || !existsSync(path.join(root, DIST_FOLDER, "index.html")))) await buildLocal(root, { customBuild: false });
  await mkdir(output, { recursive: true });
  await cp(appDir(root), path.join(output, SOURCES_FOLDER), { recursive: true, filter: (source) => !skippedName(path.basename(source)) });
  await cp(path.join(root, DIST_FOLDER), path.join(output, DIST_FOLDER), { recursive: true });
  if (existsSync(path.join(root, "server"))) await cp(path.join(root, "server"), path.join(output, "server"), { recursive: true });
  await writeFile(path.join(output, SETTINGS_FILE), `${JSON.stringify(settings, null, 2)}\n`);
  await writeEntries(output, await folderFiles(root, settings));
  await writeEntries(output, await buildProjectEntriesFromDisk(appDir(root)));
  return { output, data: false };
}

/**
 * Writes the Action Contract of the Capsule in `root` into `out`: the JSON,
 * its schema and the Markdown. The project's ID and name come from
 * konae.json when there is one.
 */
export async function writeContract(root: string, out = root) {
  const read = async (file: string) => readFile(path.join(appDir(root), file), "utf8");
  let project: { id: string; name: string } | undefined;
  try {
    const settings = await json<AppSettings>(path.join(root, SETTINGS_FILE));
    project = { id: settings.id, name: settings.name };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  let actionsSource: string;
  try { actionsSource = await read("actions.json"); } catch { throw Error(`No actions.json in ${appDir(root)}.`); }
  const schemaSource = await read("schema.sql").catch(() => "");
  const { contract, warnings } = generateActionContract({ actionsSource, schemaSource, project });
  await mkdir(out, { recursive: true });
  await writeFile(path.join(out, ACTION_CONTRACT_FILE), serializeActionContract(contract));
  await writeFile(path.join(out, ACTION_CONTRACT_SCHEMA_FILE), `${JSON.stringify(actionContractSchema, null, 2)}\n`);
  await writeFile(path.join(out, ACTION_CONTRACT_MARKDOWN), renderActionContractMarkdown(contract));
  await writeActionTypes(out, contract);
  return { contract, warnings, file: path.join(out, ACTION_CONTRACT_FILE) };
}

/** Writes actions.d.ts into `out` when its text changed, so a watcher that sees the file sees a real change. */
export async function writeActionTypes(out: string, contract: ActionContract) {
  const file = path.join(out, ACTION_TYPES_FILE);
  const text = renderActionTypes(contract);
  if (await readFile(file, "utf8").catch(() => undefined) !== text) await writeFile(file, text);
}

/**
 * Writes actions.d.ts beside konae.json from app/actions.json and
 * app/schema.sql, as dev does on each start and change. When they do not parse, the last one stays.
 */
async function refreshActionTypes(root: string) {
  const actionsSource = await readFile(path.join(appDir(root), "actions.json"), "utf8").catch(() => undefined);
  if (actionsSource === undefined) return;
  const schemaSource = await readFile(path.join(appDir(root), "schema.sql"), "utf8").catch(() => "");
  try {
    await writeActionTypes(root, generateActionContract({ actionsSource, schemaSource }).contract);
  } catch (error) {
    console.warn(`${ACTION_TYPES_FILE} stays as it was: ${error instanceof Error ? error.message : error}`);
  }
}

/** Add each entry to .gitignore unless a line already names it. */
export async function ignoreInGit(root: string, entries: string[]) {
  const file = path.join(root, ".gitignore");
  let current = "";
  try { current = await readFile(file, "utf8"); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  const present = new Set(current.split(/\r?\n/).map((line) => line.trim().replace(/^\//, "").replace(/\/$/, "")));
  const missing = entries.filter((entry) => !present.has(entry.replace(/\/$/, "")));
  if (missing.length === 0) return;
  const separator = current && !current.endsWith("\n") ? "\n" : "";
  await writeFile(file, `${separator}${missing.join("\n")}\n`, { flag: "a" });
}

function slugOf(name: string) {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "app";
}

/**
 * Writes a new app folder without a Kernel: the template in app/, konae.json
 * and the files every app folder carries. The app's ID is made here; the
 * first publish registers it on the installation with project_create, which
 * takes an ID the caller chose.
 */
export async function initLocal(root: string, options: { name?: string; template?: string } = {}) {
  // The file project_create answers from, so a folder starts the same through the CLI and through MCP.
  const templates = embeddedResources?.templates ?? await readFile(path.join(toolRoot!, "kernel-rs/konae/src/project-templates.json"), "utf8");
  const all = JSON.parse(templates) as ProjectTemplates;
  const born = all.templates[options.template ?? all.default];
  if (!born) throw Error(`Unknown template ${options.template}. Choose one of: ${INIT_TEMPLATES.join(", ")}.`);
  await assertEmptyFolder(root);
  const name = options.name ?? "My app";
  for (const [filename, source] of Object.entries(born.files)) {
    const target = path.resolve(appDir(root), filename);
    if (!target.startsWith(path.resolve(appDir(root)) + path.sep)) throw Error("Invalid template path.");
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, source, { flag: "wx" });
  }
  // The template says who may open the app. Only people the owner invites can create an account.
  const modules = defaultModuleStates();
  if (Object.values(born.files).some((source) => /miniapp\s*\.\s*files/.test(source))) modules.files = { on: true, fileAccess: null };
  if (sourcesUseHttp(born.files)) modules.http = { on: true };
  if (sourcesUseCalls(born.files)) modules.calls = { on: true };
  const settings: AppSettings = {
    id: `app_${crypto.randomUUID()}`, name, slug: slugOf(name), visibility: born.visibility,
    signupPolicy: { mode: INIT_SIGNUP_POLICY.mode, allowlist: [] }, signInMethods: ["password"], kernel: KERNEL_VERSION, modules, manifest: { version: 1 },
  };
  await writeFile(path.join(root, SETTINGS_FILE), `${JSON.stringify(settings, null, 2)}\n`, { flag: "wx" });
  await writeEntries(root, await folderFiles(root, settings));
  await ignoreInGit(root, gitignore.trim().split("\n"));
  return settings;
}

/** The Kernel `dev` runs on the folder, and the commands `sql`, `events` and `dev` give it. */
export type LocalKernel = { binary: string; env: Record<string, string | undefined>; data: string; mailbox: string; run(args: string[], input?: string): Promise<string> };

async function localStateSummary(root: string) {
  return localResetSummary(await json<unknown>(path.join(root, ".konae/local-state.json")).catch(() => undefined));
}

/**
 * 1 TiB, the quota a local Kernel holds unless KONAE_FILES_QUOTA_BYTES names another.
 * `kernel import` checks pulled files against the quota and the per-file limit, and
 * a published app's files can exceed the 20 MiB default quota, so `dev --reset` needs room.
 */
const LOCAL_FILES_QUOTA = String(1024 ** 4);

/** Locates a compatible Kernel without creating an app runtime. */
async function resolveLocalKernel(options: { build?: boolean } = {}) {
  let binary = process.env.KONAE_KERNEL_BIN ? path.resolve(process.env.KONAE_KERNEL_BIN) : toolRoot ? path.join(toolRoot, "kernel-rs/target/debug/kernel") : "";
  if (!process.env.KONAE_KERNEL_BIN && toolRoot && options.build !== false && !existsSync(binary)) {
    console.error("Building the Kernel with cargo. The first build takes about a minute.");
    const compilation = Bun.spawn(["cargo", "build", "--manifest-path", path.join(toolRoot, "kernel-rs/Cargo.toml"), "-p", "kernel"], { stdout: "inherit", stderr: "inherit" });
    if (await compilation.exited !== 0) throw Error("Could not build the Kernel. Install Rust, or set KONAE_KERNEL_BIN to a kernel binary built from this checkout.");
  }
  if (!process.env.KONAE_KERNEL_BIN && !existsSync(binary)) binary = await downloadKernel(KERNEL_VERSION);
  if (!existsSync(binary)) throw Error(`No kernel binary at ${binary}. Run dev once, or set KONAE_KERNEL_BIN.`);
  const capabilities = await requireTestKernel(binary);
  return { binary, capabilities };
}

/** Reads the source schema or local data structure without opening an app runtime. */
export async function fingerprintLocal(root: string, data = false): Promise<string> {
  const { binary, capabilities } = await resolveLocalKernel();
  if (capabilities.schemaTransforms !== 1) throw Error("This Kernel does not support schema fingerprints. Update the Kernel or set KONAE_KERNEL_BIN to a compatible binary.");
  const file = path.join(root, data ? "data/database.sqlite" : "app/schema.sql");
  const child = Bun.spawn([binary, "schema", "fingerprint", data ? "--database" : "--schema", file], { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (code !== 0) throw Error(error.trim().replace(/^Error: /, "") || "Could not fingerprint the schema.");
  const answer = JSON.parse(output) as { fingerprint?: unknown };
  if (typeof answer.fingerprint !== "string" || !/^sha256:[a-f0-9]{64}$/.test(answer.fingerprint)) throw Error("Kernel returned an invalid schema fingerprint.");
  return answer.fingerprint;
}

/**
 * The `kernel` binary on this machine and the environment that points it at
 * the folder. Its data directory is `.konae/kernel`, where `apps/<id>` links
 * back to the folder, so the Kernel reads `konae.json`, `app/`, `dist/` and
 * `data/` where they are. KONAE_KERNEL_BIN names a built `kernel` binary to
 * run instead of compiling one.
 */
export async function localKernel(root: string, settings: AppSettings, options: { port?: string; confirmSchema?: string; build?: boolean } = {}): Promise<LocalKernel> {
  const { binary } = await resolveLocalKernel(options);
  const data = path.join(root, ".konae/kernel");
  const link = path.join(data, "apps", settings.id);
  await mkdir(path.dirname(link), { recursive: true });
  if (await readlink(link).catch(() => undefined) === undefined) await symlink(root, link, "dir");
  await ignoreInGit(root, [".konae/"]);
  const port = options.port ?? "8796";
  const env: Record<string, string | undefined> = {
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_") && !name.startsWith("VITE_"))),
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => [
      "KONAE_FUNCTIONS_RUNNER", "KONAE_FUNCTIONS_RUNNER_KIND", "KONAE_FUNCTIONS_DEVELOPMENT", "KONAE_FUNCTIONS_CGROUP",
      "KONAE_FUNCTIONS_UID", "KONAE_FUNCTIONS_GID", "KONAE_FUNCTIONS_PREPARER",
      "KONAE_FUNCTIONS_WASM_CACHE", "KONAE_FUNCTIONS_WASM_CACHE_BYTES",
    ].includes(name))),
    KONAE_BIND: "127.0.0.1", KONAE_DATA_DIR: data, KONAE_KERNEL_PORT: port,
    KONAE_PUBLIC_ORIGIN: `http://localhost:${port}`, KONAE_HOST_SUFFIX: "localhost",
    KONAE_EXECUTION_MODE: "test",

    KONAE_MAIL_TRANSPORT: "development",

    KONAE_FILES_QUOTA_BYTES: process.env.KONAE_FILES_QUOTA_BYTES || LOCAL_FILES_QUOTA,
    ...(process.env.KONAE_FILE_MAX_BYTES ? { KONAE_FILE_MAX_BYTES: process.env.KONAE_FILE_MAX_BYTES } : {}),

    ...(process.env.KONAE_SCHEDULE_MIN_SECONDS ? { KONAE_SCHEDULE_MIN_SECONDS: process.env.KONAE_SCHEDULE_MIN_SECONDS } : {}),
    ...(options.confirmSchema ? { KONAE_CONFIRM_SCHEMA: options.confirmSchema } : {}),
  };
  const run = async (args: string[], input?: string) => {
    const mutatesTestState = args[0] === "test" && (args[1] === "reply" || args[1] === "schedule");
    const release = mutatesTestState ? await acquireLocalControlLock(root) : undefined;
    let child: ReturnType<typeof Bun.spawn> | undefined;
    let interrupted: "SIGINT" | "SIGTERM" | undefined;
    const onSignal = (signal: "SIGINT" | "SIGTERM") => {
      interrupted = signal;
      shutdownSignalCode = signal === "SIGINT" ? 130 : 143;
      process.exitCode = shutdownSignalCode;
      child?.kill(signal);
    };
    const onSigint = () => onSignal("SIGINT");
    const onSigterm = () => onSignal("SIGTERM");
    if (mutatesTestState) {
      process.once("SIGINT", onSigint);
      process.once("SIGTERM", onSigterm);
    }
    try {
      child = Bun.spawn([binary, ...args, "--app", settings.id], { env, stdin: input === undefined ? "ignore" : new Blob([input]), stdout: "pipe", stderr: "pipe" });
      const [code, output, error] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      if (interrupted) throw Error(`Local ${args[1]} control stopped by ${interrupted}.`);
      if (code !== 0) throw Error(error.trim().replace(/^Error: /, "") || `kernel ${args[0]} stopped with ${code}.`);
      return output;
    } finally {
      await release?.();
      if (mutatesTestState) {
        process.removeListener("SIGINT", onSigint);
        process.removeListener("SIGTERM", onSigterm);
      }
    }
  };
  return { binary, env, data, mailbox: path.join(data, "mail-testbox.jsonl"), run };
}

const KERNEL_RELEASE = "https://github.com/danibram/konae/releases/download";

/** Downloads this CLI release's Kernel and checks its published SHA-256. */
export async function downloadKernel(version: string): Promise<string> {
  const os = process.platform === "darwin" ? "darwin" : process.platform === "linux" ? "linux" : undefined;
  const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : undefined;
  if (!os || !arch) throw Error(`No Kernel binary is published for ${process.platform}/${process.arch}. Set KONAE_KERNEL_BIN to a compatible kernel.`);
  const name = `kernel-${version}-${os}-${arch}`;
  const cache = process.env.KONAE_CACHE_DIR ?? path.join(process.env.XDG_CACHE_HOME ?? path.join(process.env.HOME ?? tmpdir(), ".cache"), "konae");
  const directory = path.join(cache, "kernels", version);
  const binary = path.join(directory, name);
  const sumsFile = path.join(directory, "SHA256SUMS");
  const valid = async () => {
    try {
      const expected = (await readFile(sumsFile, "utf8")).split("\n").map((line) => line.trim().split(/\s+/)).find(([, file]) => file === name)?.[0];
      if (!expected) return false;
      return createHash("sha256").update(await readFile(binary)).digest("hex") === expected;
    } catch { return false; }
  };
  if (await valid()) return binary;
  await mkdir(directory, { recursive: true });
  const url = `${KERNEL_RELEASE}/v${version}`;
  const [sumsResponse, binaryResponse] = await Promise.all([fetch(`${url}/SHA256SUMS`), fetch(`${url}/${name}`)]);
  if (!sumsResponse.ok) throw Error(`Could not download ${name} checksums from ${url} (HTTP ${sumsResponse.status}). Set KONAE_KERNEL_BIN to a kernel binary from Konae ${version}.`);
  if (!binaryResponse.ok) throw Error(`Could not download ${name} from ${url} (HTTP ${binaryResponse.status}). Set KONAE_KERNEL_BIN to a kernel binary from Konae ${version}.`);
  const sums = await sumsResponse.text();
  const expected = sums.split("\n").map((line) => line.trim().split(/\s+/)).find(([, file]) => file === name)?.[0];
  if (!expected || !/^[a-f0-9]{64}$/i.test(expected)) throw Error(`SHA256SUMS from ${url} has no valid entry for ${name}.`);
  const bytes = new Uint8Array(await binaryResponse.arrayBuffer());
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected.toLowerCase()) throw Error(`${name} failed SHA-256 verification. Expected ${expected}, got ${actual}.`);
  const temporary = `${binary}.download-${crypto.randomUUID()}`;
  await writeFile(temporary, bytes, { flag: "wx" });
  try {
    await writeFile(sumsFile, sums);
    await chmod(temporary, 0o755);
    await rename(temporary, binary);
  } finally {
    await rm(temporary, { force: true });
  }
  return binary;
}

/**
 * Sets a local value for each signing secret actions.json names that the
 * local app does not hold yet, and returns one line for each secret it set.
 * The Kernel does not serve an app whose route signs with an unset secret,
 * and the published app's values never leave its installation.
 */
export async function setDevSecrets(root: string, kernel: Pick<LocalKernel, "run">) {
  let actions: Record<string, { http?: { signature?: { secret?: string } }; auth?: { secret?: string } }>;
  try { actions = JSON.parse(await readFile(path.join(appDir(root), "actions.json"), "utf8")).actions ?? {}; } catch { return []; }
  const wanted = new Map<string, "route" | "call">();
  for (const action of Object.values(actions)) {
    if (typeof action?.http?.signature?.secret === "string") wanted.set(action.http.signature.secret, "route");
    if (typeof action?.auth?.secret === "string") wanted.set(action.auth.secret, "call");
  }
  if (wanted.size === 0) return [];
  const held = new Set((JSON.parse(await kernel.run(["secret", "list"])) as { secrets?: { name: string }[] }).secrets?.map((secret) => secret.name));
  const lines: string[] = [];
  for (const [name, use] of wanted) {
    if (held.has(name)) continue;
    // whsec_ and base64, so a svix route takes it too.
    const value = `whsec_${Buffer.from(crypto.getRandomValues(new Uint8Array(24))).toString("base64")}`;
    await kernel.run(["secret", "set", name, "--use", use], value);
    lines.push(`Set the secret ${name} of the local app to ${value}. Sign local requests with it; the published app keeps its own.`);
  }
  return lines;
}

/** The local app's record of a range, from one run of the kernel binary's events command. */
export function kernelRecord(kernel: Pick<LocalKernel, "run">): ReadRecord {
  return async ({ from, to }) => JSON.parse(await kernel.run(["events", "--from", from, "--to", to]));
}

/**
 * Replaces data/ with the data `pull` exported into .konae/pull, through
 * the kernel binary's import, or empties it when there is none. `dev
 * --reset` runs it after the build and before the Kernel starts.
 */
export async function acquireLocalLifecycleLock(root: string): Promise<() => Promise<void>> {
  const file = path.join(root, ".konae/lifecycle.lock");
  await mkdir(path.dirname(file), { recursive: true });
  try {
    const handle = await open(file, "wx", 0o600);
    await handle.writeFile(JSON.stringify({ pid: process.pid, at: Date.now() }));
    await handle.close();
    return async () => { await rm(file, { force: true }); };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    const owner = await json<{ pid?: number }>(file).catch(() => undefined);
    if (owner?.pid && processExists(owner.pid)) throw Error(`A local Kernel process (pid ${owner.pid}) holds ${file}. Stop it before resetting app data.`);
    throw Error(`A local lifecycle lock at ${file} needs operator recovery. Check that no konae dev process is running, then remove the stale lock.`);
  }
}

/** Serializes test replies and schedules with local data replacement. */
export async function acquireLocalControlLock(root: string): Promise<() => Promise<void>> {
  const file = path.join(root, ".konae/control.lock");
  await mkdir(path.dirname(file), { recursive: true });
  let created = false;
  let handle: Awaited<ReturnType<typeof open>> | undefined;
  try {
    handle = await open(file, "wx", 0o600);
    created = true;
    await handle.writeFile(JSON.stringify({ pid: process.pid, at: Date.now() }));
    await handle.close();
    handle = undefined;
    return async () => { await rm(file, { force: true }); };
  } catch (error) {
    await handle?.close().catch(() => {});
    if (created) await rm(file, { force: true }).catch(() => {});
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    const owner = await json<{ pid?: number }>(file).catch(() => undefined);
    if (owner?.pid && processExists(owner.pid)) throw Error(`A local test control or reset (pid ${owner.pid}) holds ${file}. Wait for it to finish before running another test control or reset.`);
    throw Error(`A local control lock at ${file} needs operator recovery. Check that no local test control or reset is running, then remove the stale lock.`);
  }
}

function processExists(pid: number) {
  try { process.kill(pid, 0); return true; }
  catch (error) { return (error as NodeJS.ErrnoException).code === "EPERM"; }
}

type LocalResetRecord = { snapshotId: string; executionId: string; packageId?: string; resetAt: number; backup: string; callsRetired: number; mailRetired: number };

export async function resetData(root: string, kernel: Pick<LocalKernel, "run" | "data">, pulled: Pulled | undefined, confirmSchema?: string, options: { lockHeld?: boolean } = {}) {
  const releaseLifecycle = options.lockHeld ? undefined : await acquireLocalLifecycleLock(root);
  let releaseControl: (() => Promise<void>) | undefined;
  try {
    releaseControl = await acquireLocalControlLock(root);
    const executionId = crypto.randomUUID();
    const snapshotId = pulled?.snapshotId ?? crypto.randomUUID();
    const backup = path.join(root, ".konae/backups", `local-reset-${executionId}`);
    await snapshotLocalState(root, kernel.data, backup);
    try {
      if (pulled) {
        const importRoot = path.join(root, ".konae", `reset-import-${executionId}`);
        let imported: { backup?: { id?: string } };
        try {
          await extractVerifiedPulledData(root, importRoot, pulled);
          imported = JSON.parse(await kernel.run(["import", importRoot, ...(confirmSchema ? ["--confirm-schema", confirmSchema] : [])])) as { backup?: { id?: string } };
        } finally { await rm(importRoot, { recursive: true, force: true }); }
        if (!imported.backup?.id) throw Error("The Kernel imported the data without returning its backup record.");
      } else {
        await emptyReset(root);
      }
      const effects = JSON.parse(await kernel.run(["test", "reset-effects"])) as { callsRetired: number; mailRetired: number };
      if (!Number.isSafeInteger(effects.callsRetired) || !Number.isSafeInteger(effects.mailRetired)) throw Error("The Kernel returned an invalid test reset-effects result.");
      if (pulled) await writeFile(path.join(root, IMPORTED_MARKER), JSON.stringify({ packageId: pulled.manifest.packageId, snapshotId, executionId }));
      const record: LocalResetRecord = { snapshotId, executionId, ...(pulled ? { packageId: pulled.manifest.packageId } : {}), resetAt: Date.now(), backup, callsRetired: effects.callsRetired, mailRetired: effects.mailRetired };
      await writeFile(path.join(root, ".konae/local-state.json"), JSON.stringify(record, null, 2), { mode: 0o600 });
      return record;
    } catch (error) {
      try { await restoreLocalState(root, kernel.data, backup); }
      catch (restoreError) { throw Error(`Reset failed (${error instanceof Error ? error.message : String(error)}); restoring its data and delivery queues also failed (${restoreError instanceof Error ? restoreError.message : String(restoreError)}). Backup ${backup} remains available.`); }
      throw error;
    }
  } finally {
    await releaseControl?.();
    await releaseLifecycle?.();
  }
}

async function snapshotLocalState(root: string, kernelData: string, backup: string) {
  await mkdir(path.join(backup, "queues"), { recursive: true });
  const data = path.join(root, DATA_FOLDER);
  if (await exists(data)) await cp(data, path.join(backup, DATA_FOLDER), { recursive: true, errorOnExist: true });
  const queueFiles: string[] = [];
  for (const name of ["mail-test.sqlite", "mail.sqlite", "calls.sqlite"]) {
    for (const suffix of ["", "-wal", "-shm", "-journal"]) {
      const source = path.join(kernelData, name + suffix);
      if (await exists(source)) {
        await cp(source, path.join(backup, "queues", name + suffix));
        queueFiles.push(name + suffix);
      }
    }
  }
  if (await exists(path.join(kernelData, "mail-testbox.jsonl"))) {
    await cp(path.join(kernelData, "mail-testbox.jsonl"), path.join(backup, "queues/mail-testbox.jsonl"));
    queueFiles.push("mail-testbox.jsonl");
  }
  for (const marker of [IMPORTED_MARKER, ".konae/local-state.json"]) {
    if (await exists(path.join(root, marker))) await cp(path.join(root, marker), path.join(backup, path.basename(marker)));
  }
  await writeFile(path.join(backup, "snapshot.json"), JSON.stringify({ createdAt: Date.now(), dataPresent: await exists(data), queueFiles }, null, 2), { mode: 0o600 });
}

async function restoreLocalState(root: string, kernelData: string, backup: string) {
  const data = path.join(root, DATA_FOLDER);
  const staged = path.join(root, `.data-restore-staged-${crypto.randomUUID()}`);
  const record = JSON.parse(await readFile(path.join(backup, "snapshot.json"), "utf8")) as { dataPresent: boolean; queueFiles: string[] };
  if (record.dataPresent) await cp(path.join(backup, DATA_FOLDER), staged, { recursive: true, errorOnExist: true });
  if (await exists(data)) await rm(data, { recursive: true, force: true });
  if (record.dataPresent) await rename(staged, data);
  for (const name of ["mail-test.sqlite", "mail.sqlite", "calls.sqlite"]) {
    for (const suffix of ["", "-wal", "-shm", "-journal"]) await rm(path.join(kernelData, name + suffix), { force: true });
  }
  for (const file of record.queueFiles) await cp(path.join(backup, "queues", file), path.join(kernelData, file));
  for (const [saved, target] of [["pull-imported.json", IMPORTED_MARKER], ["local-state.json", ".konae/local-state.json"]]) {
    const destination = path.join(root, target);
    const source = path.join(backup, saved);
    if (await exists(source)) await cp(source, destination);
    else await rm(destination, { force: true });
  }
}

async function emptyReset(root: string) {
  const data = path.join(root, DATA_FOLDER);
  const staged = path.join(root, `.data-reset-staged-${crypto.randomUUID()}`);
  await mkdir(staged, { recursive: true });
  try {
    if (await exists(path.join(data, "secrets.sqlite"))) await cp(path.join(data, "secrets.sqlite"), path.join(staged, "secrets.sqlite"));
    const retired = path.join(root, `.data-reset-retired-${crypto.randomUUID()}`);
    if (await exists(data)) await rename(data, retired);
    await rename(staged, data);
    await rm(retired, { recursive: true, force: true });
  } finally { await rm(staged, { recursive: true, force: true }); }
}

async function exists(file: string) { try { await stat(file); return true; } catch { return false; } }

/** Which export of the published app's data `dev --reset` last put in data/. */
const IMPORTED_MARKER = ".konae/pull-imported.json";

/** Returns the current pull until dev imports the same package into data/. */
export async function pendingPull(root: string): Promise<Pulled | undefined> {
  const pulled = await readPulled(root);
  if (!pulled) return undefined;
  const imported = await json<{ packageId?: string }>(path.join(root, IMPORTED_MARKER)).catch(() => undefined);
  return imported?.packageId === pulled.manifest.packageId ? undefined : pulled;
}

/** init and export write a whole app folder, so the target must hold nothing but dotfiles (.git, .konae, .gitignore). */
async function assertEmptyFolder(root: string) {
  let entries: string[] = [];
  try { entries = await readdir(root); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  const occupied = entries.filter((name) => !name.startsWith("."));
  if (occupied.length > 0) throw Error(`Refusing to overwrite ${occupied.slice(0, 3).join(", ")}. Choose an empty directory.`);
}

type DomainView = {
  hostname: string | null; status: "none" | "pending" | "active"; url?: string; generatedUrl: string;
  record?: { type: "TXT"; name: string; value: string }; dns?: string; rules?: DomainRules;
};

/** domain_read, domain_set or domain_clear, with the hostname checked by the Kernel's rules first. Returns the lines to print. */
export async function domainLocal(client: Pick<McpClient, "call">, deploymentId: string, request: { set?: string; clear?: boolean }): Promise<string[]> {
  if (request.clear) {
    const cleared = await client.call<{ cleared: string | null; generatedUrl: string }>("domain_clear", { projectId: deploymentId });
    return [cleared.cleared ? `${cleared.cleared} no longer serves this app. ${cleared.generatedUrl} keeps answering.` : `This app had no custom domain. It answers at ${cleared.generatedUrl}.`];
  }
  const current = await client.call<DomainView>("domain_read", { projectId: deploymentId });
  let view = current;
  if (request.set !== undefined) {
    const checked = checkHostname(request.set, current.rules ?? { suffix: "", installationHosts: [] });
    if (!checked.ok) throw Error(checked.message);
    view = await client.call<DomainView>("domain_set", { projectId: deploymentId, hostname: checked.hostname, expected: current.hostname });
  }
  if (view.status === "none") return [`This app has no custom domain. It answers at ${view.generatedUrl}.`];
  if (view.status === "active") return [`${view.url} serves this app, next to ${view.generatedUrl}. Your proxy must terminate TLS for ${view.hostname} and forward Host unchanged.`];
  return [
    `${view.hostname} does not serve yet.${view.dns ? ` ${view.dns}` : ""}`,
    `Add this record at your DNS provider: ${view.record!.type} ${view.record!.name} "${view.record!.value}"`,
    `Point ${view.hostname} at this installation's proxy, then run this command again with --set ${view.hostname}.`,
  ];
}


export function toolArguments(text?: string, piped?: { key: string; value: string }): Record<string, unknown> {
  let value: unknown = {};
  if (text !== undefined) {
    try { value = JSON.parse(text); } catch { value = undefined; }
    if (!value || typeof value !== "object" || Array.isArray(value)) throw Error(`The arguments must be one JSON object, such as '{"projectId":"app_1"}', not ${text}.`);
  }
  const arguments_ = value as Record<string, unknown>;
  if (piped) arguments_[piped.key] = piped.value.replace(/[\r\n]+$/, "");
  return arguments_;
}

export async function main() {
  const argv = process.argv.slice(2);
  if (argv[0] === "--") argv.shift();
  const [command, ...args] = argv;
  if (!command || ["help", "--help"].includes(command)) {
    console.log(HELP);
    return;
  }
  if (SERVER_COMMANDS.includes(command)) throw Error(serverCommandMessage(command));
  if (!["login", "init", "build", "contract", "conform", "export", "publish", "dev", "pull", "rehearse", "schema", "sql", "events", "test", "domain", "hostname", "push", "secret", "tool"].includes(command)) throw Error(`Unknown command ${command}.\n\n${HELP}`);
  const option = (name: string, fallback?: string) => {
    const i = args.indexOf(`--${name}`);
    if (i < 0) return fallback;
    const value = args[i + 1];
    if (value === undefined || value.startsWith("--")) throw Error(`--${name} needs a value.`);
    return value;
  };
  const root = path.resolve(option("dir", process.cwd())!);
  if (command === "schema") {
    if (args[0] !== "fingerprint") throw Error("Use schema fingerprint --dir <folder> [--data].");
    const fingerprint = await fingerprintLocal(root, args.includes("--data"));
    console.log(args.includes("--json") ? JSON.stringify({ fingerprint, source: args.includes("--data") ? "data" : "schema" }) : fingerprint);
    return;
  }
  if (command === "rehearse") {
    const settings = await readSettings(root);
    await refreshActionTypes(root);
    await buildLocal(root, { development: true });
    const result = await rehearsePulledSnapshot(root, settings, async (scratch, snapshot) => {
      const kernel = await localKernel(scratch, settings);
      return importWithLocalKernel(kernel, snapshot, option("confirm-schema"));
    });
    console.log(JSON.stringify({ ...result, source: "current candidate against konae pull data", rehearsal: "passed" }, null, 2));
    return;
  }
  if (command === "login") {
    const url = option("url", process.env.KONAE_URL ?? "http://localhost:4173")!;
    const scopes = loginScopes(args);
    let token: string; let saved = "Saved a 24-hour MCP credential in .konae/credentials.json. Keep .konae out of Git.";
    if (args.includes("--browser")) {
      const signedIn = await browserLogin(url, scopes, { open: !args.includes("--no-open") });
      token = signedIn.token;
      saved = `Saved a ${lifetimeLabel(signedIn.expiresIn)} MCP credential with ${scopes.join(", ")} in .konae/credentials.json. Keep .konae out of Git.`;
    } else {
      const { email, password } = await loginCredentials(process.env, process.stdin);
      const verifyFrom = option("verify-from");
      token = await login(url, email, password, { code: process.env.KONAE_INVITATION_CODE, setupKey: process.env.KONAE_SETUP_KEY }, scopes, verifyFrom && path.resolve(verifyFrom));
    }
    await mkdir(path.join(root, ".konae"), { recursive: true });
    await writeFile(path.join(root, ".konae/credentials.json"), JSON.stringify({ url, token }), { mode: 0o600 });
    await chmod(path.join(root, ".konae/credentials.json"), 0o600);
    await ignoreInGit(root, [".konae/"]);
    console.log(saved);
    return;
  }
  if (command === "init") {
    const settings = await initLocal(root, { name: option("name"), template: option("template") });
    console.log(`Created ${settings.name} in ${root}. Run it with: konae dev --dir ${root}`);
    return;
  }
  if (command === "build") { await buildLocal(root, { customBuild: true }); console.log(`Built ${path.join(root, DIST_FOLDER)}`); return; }
  if (command === "contract") {
    const { contract, warnings, file } = await writeContract(root, path.resolve(option("out", root)!));
    for (const warning of warnings) console.warn(`Warning: ${warning.message}`);
    console.log(`Wrote the Action Contract for ${contract.actions.length} action${contract.actions.length === 1 ? "" : "s"} to ${file}, with its schema and Markdown.`);
    return;
  }
  if (command === "conform") {
    const { conformFromCli } = await import("./conform");
    process.exitCode = await conformFromCli(args);
    return;
  }
  // The installation every remote command talks to: --url, then the one login saved beside the token it would send.
  const installation = async () => {
    const url = option("url") ?? (await json<{ url?: string }>(path.join(root, ".konae/credentials.json")).catch(() => undefined))?.url;
    if (!url) throw Error(NO_INSTALLATION);
    return url;
  };
  // The owner's MCP on that installation, with KONAE_MCP_TOKEN or the token login saved.
  const ownerClient = async () => new McpClient(endpoint(await installation()), { token: await credentials(root) });
  if (command === "domain" || command === "hostname") {
    // konae.json's id is the Deployment's ID, so a folder names the Deployment without --deployment.
    const deploymentId = option("deployment") ?? (args.includes("--dir") ? (await readSettings(root)).id : undefined);
    if (!deploymentId) throw Error(DOMAIN_NEEDS_DEPLOYMENT);
    const client = await ownerClient();
    const lines = command === "hostname" ? await hostnameLocal(client, deploymentId, option("set")) : await domainLocal(client, deploymentId, { set: option("set"), clear: args.includes("--clear") });
    for (const line of lines) console.log(line);
    return;
  }
  if (command === "tool") {
    // The name and the JSON are the two words that are neither a flag nor a flag's value.
    const [name, text] = args.filter((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--stdin"].includes(args[i - 1]));
    if (!name) throw Error("Name the tool: tool <name> ['<json arguments>']. docs/mcp-tools.md lists the tools, and tool session_inspect shows what this credential may call.");
    const key = option("stdin");
    const piped = key === undefined ? undefined : { key, value: await Bun.stdin.text() };
    console.log(JSON.stringify(await (await ownerClient()).call(name, toolArguments(text, piped)), null, 2));
    return;
  }
  const config = configOf(await readSettings(root));
  if (command === "export") {
    const out = option("out");
    if (!out) throw Error("Name an empty folder for the export with --out <folder>.");
    const withData = args.includes("--with-data");
    const exported = await exportLocal(root, path.resolve(out), withData ? { url: await installation(), token: await credentials(root) } : undefined);
    console.log(`Exported the app folder to ${exported.output}${exported.data ? ", with the published app's data in data/" : ""}. Its secrets stay where they are set.`);
    return;
  }
  if (command === "publish") {
    const url = await installation();
    const planned = args.includes("--plan");
    const result = await publishLocal(root, { ...config, url }, await credentials(root), console.warn, confirmOnTerminal(option("confirm-schema")), planned);
    if (planned) {
      if (args.includes("--json")) console.log(JSON.stringify(result, null, 2));
      else {
        const value = result as Awaited<ReturnType<typeof publishLocal>>;
        console.log(`Plan for ${value.target.projectName} (${value.target.projectId}) at ${value.target.installation}`);
        console.log(`Sources ${value.sourceDigest}; artifact ${value.artifactDigest}; remote sources revision ${value.sourceRevision.planned}.`);
        console.log(value.expiresAt === null ? "This local-only baseline has no remote plan ID or expiry." : `Plan expires ${value.expiresAt}.`);
        if (value.plan.schema) {
          console.log(describeSchemaChange(value.plan.schema).join("\n"));
        }
        for (const requirement of value.productionRequirements) console.log(requirement);
      }
    } else console.log((result as { url: string }).url);
    return;
  }
  if (command === "secret") {
    const [action, ...rest] = args;
    const projectId = config.project.id;
    const client = async () => new McpClient(endpoint(await installation()), { token: await credentials(root) });
    if (action === "list") {
      const listed = await (await client()).call<{ secrets: { name: string; use: string; updatedAt: number }[] }>("secrets_list", { projectId });
      if (!listed.secrets.length) console.log(`${projectId} has no secrets.`);
      for (const secret of listed.secrets) console.log(`${secret.name}  use ${secret.use}  set ${new Date(secret.updatedAt).toISOString()}`);
      return;
    }
    if (action !== "set") throw Error("Use secret set <name> or secret list.");
    // The name is the one argument that is neither a flag nor a flag's value. The value never comes through argv.
    const name = rest.find((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--use"].includes(rest[i - 1]));
    if (!name) throw Error("Name the secret: secret set <name>, then give its value on stdin.");
    const use = option("use");
    if (use && !(SECRET_USES as readonly string[]).includes(use)) throw Error(`--use takes ${SECRET_USES.join(", ")}.`);
    const value = process.stdin.isTTY ? await readSecret(`Value for ${name} (not shown): `, process.stdin, process.stderr) : pipedSecret(await Bun.stdin.text());
    if (!value) throw Error(`No value for ${name}, so nothing was set. Type it at the prompt, or pipe it in: <command that prints it> | konae secret set ${name}`);
    console.log(await setSecret(await client(), projectId, name, value, use));
    return;
  }
  // sql and events read the local app through the Kernel's commands, and the published Deployment only with --prod.
  const target = async (): Promise<{ deploymentId: string; feed?: string } & ({ local: LocalKernel; client?: undefined } | { client: McpClient; local?: undefined })> => {
    if (!args.includes("--prod")) {
      const settings = await readSettings(root);
      const local = await localKernel(root, settings, { build: false });
      const session = await json<{ events?: string }>(path.join(root, DEV_SESSION)).catch(() => undefined);
      return { deploymentId: settings.id, local, feed: session?.events };
    }
    const client = await ownerClient();
    const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId: config.project.id });
    if (!remote.deployment) throw Error("This project has no Deployment yet. Publish it first.");
    return { client, deploymentId: remote.deployment.id };
  };
  if (command === "sql") {
    const file = option("file");
    // The statement is the one argument that is neither a flag nor a flag's value.
    const inline = args.find((arg, i) => !arg.startsWith("--") && !["--dir", "--url", "--file"].includes(args[i - 1]));
    const sql = file ? await readFile(path.resolve(file), "utf8") : inline;
    if (!sql?.trim()) throw Error("Give the statement in quotes or with --file <file.sql>.");
    const found = await target();
    const answer = found.local ? JSON.parse(await found.local.run(["sql", sql])) : await found.client.call("database_execute", { projectId: found.deploymentId, operation: "sql", input: { sql } });
    console.log(JSON.stringify(answer, null, 2));
    return;
  }
  if (command === "test") {
    if (args.includes("--prod")) throw Error("konae test only runs against the local test Kernel.");
    const positional = args.filter((arg, index) => !arg.startsWith("--") && !["--dir", "--file", "--url"].includes(args[index - 1]));
    const [action, ...rest] = positional;
    const settings = await readSettings(root);
    const kernel = await localKernel(root, settings, { build: false });
    let result: unknown;
    if (action === "status" && rest.length === 0) result = await testControls(kernel, () => localStateSummary(root)).status();
    else if (action === "calls" && rest.length === 0) result = await testCalls(kernel);
    else if (action === "schedule" && rest.length === 1) result = await testSchedule(kernel, rest[0]);
    else if (action === "reply" && rest.length === 1) {
      const file = option("file");
      const stdinFlag = args.includes("--stdin");
      if (file && stdinFlag) throw Error("Choose --stdin or --file <json>, not both.");
      if (!file && !stdinFlag) throw Error("Reply JSON must come from --stdin or --file <json>.");
      const body = file ? await readReplyFile(path.resolve(file)) : await readReplyInput(Bun.stdin.stream());
      result = await testReply(kernel, rest[0], body);
    } else if (action === "scenario" && rest.length === 1) {
      const scenario = await resolveScenarioPath(root, path.resolve(root, rest[0]));
      result = await runScenario(kernel, scenario);
    } else throw Error("Use konae test status, calls, reply <call-id> --stdin|--file <json>, schedule <action>, or scenario <file>.");
    console.log(JSON.stringify(result, null, 2));
    if (action === "reply" && result && typeof result === "object" && (result as { accepted?: unknown }).accepted === false) process.exitCode = 1;
    return;
  }
  if (command === "events") {
    const { deploymentRecord, formatEvent, readEvents } = await import("./events");
    const found = await target();
    const { deploymentId, local, feed } = found;
    const record = found.local ? kernelRecord(found.local) : deploymentRecord(found.client, deploymentId);
    const days = Number(option("days", "7"));
    if (!Number.isInteger(days) || days < 1 || days > 400) throw Error("--days takes a whole number from 1 to 400.");
    const asJson = args.includes("--json");
    const seen = new Map<number, number>();
    const printed = new Set<string>();
    const read = async () => {
      // Only the local Kernel has a mailbox this machine can read.
      // dev's events page stamps each mail when it is written, so its answer is preferred to a read from here.
      const served = feed ? await (async () => {
        const feedUrl = new URL(feed);
        const token = new URLSearchParams(feedUrl.hash.slice(1)).get("control") ?? "";
        feedUrl.hash = "";
        feedUrl.pathname = "/events.json";
        feedUrl.search = new URLSearchParams({ days: String(days) }).toString();
        return fetch(feedUrl, { headers: { "X-Konae-Control": token } }).then((response) => response.ok ? response.json() as Promise<AppEvent[]> : undefined).catch(() => undefined);
      })() : undefined;
      const events = served ?? await readEvents(record, deploymentId, { days, seen, mailbox: local?.mailbox });
      for (const event of events) {
        // A notice or a call changes state, so the same event prints again when its outcome does.
        const key = `${event.id}:${event.outcome}`;
        if (printed.has(key)) continue;
        printed.add(key);
        console.log(asJson ? JSON.stringify(event) : formatEvent(event));
      }
    };
    await read();
    if (!args.includes("--follow")) {
      if (printed.size === 0 && !asJson) console.log(`Nothing yet in the last ${days} days: no email, call, scheduled run, route or Signal.`);
      return;
    }
    for (;;) { await Bun.sleep(1000); await read().catch((error) => { console.error(error instanceof Error ? error.message : error); process.exit(1); }); }
  }
  if (command === "pull") {
    const url = await installation();
    const pulled = await pullLocal(root, new McpClient(endpoint(url), { token: await credentials(root) }), url, config.project.id, { dataOnly: args.includes("--data-only") });
    console.log(`${pulled.dataOnly ? "Exported data" : "Exported app/, dist/ and data"} ${describePulled(pulled)} to ${path.join(root, PULL_DIRECTORY)}. It holds the app's accounts and rows, so keep .konae out of Git.`);
    if (!pulled.dataOnly && !pulled.sourcesInstalled) console.log(`Local app/ or konae.json has edits or no recorded base, so pull kept both. Their saved copies are staged in ${path.join(root, PULL_DIRECTORY)}. Copy app/ and konae.json from there into the folder when you are ready, then run pull again to record the new source revision.`);
    console.log(`Import it into the local app with: konae dev --dir ${root} --reset`);
    return;
  }
  if (command === "push") {
    if (await pendingPull(root)) throw Error(`Exported data waits in ${path.join(root, PULL_DIRECTORY)}. Run dev --reset before push so the folder's data includes that import.`);
    const settings = await readSettings(root);
    const pushed = await pushData(root, new McpClient(endpoint(await installation()), { token: await credentials(root) }), { id: settings.id, name: settings.name, visibility: settings.visibility },
      { yes: args.includes("--yes"), confirmSchema: option("confirm-schema") });
    console.log(`Pushed. To undo it, restore backup ${pushed.backupId} with database_execute { projectId: "${pushed.deploymentId}", operation: "restore-backup", input: { backupId: "${pushed.backupId}" } }.`);
    return;
  }
  if (command === "dev") {
    const settings = await readSettings(root);
    const port = option("port", "8796")!;
    if (!/^\d+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) throw Error("Choose a local port between 1024 and 65535.");
    const kernel = await localKernel(root, settings, { port, confirmSchema: option("confirm-schema") });
    const pulled = await readPulled(root);
    let reset = args.includes("--reset");
    if (!reset && await pendingPull(root)) {
      console.log(`Exported data waits in ${path.join(root, PULL_DIRECTORY)}. Run dev with --reset to replace data/ with it.`);
    }
    const devSession = path.join(root, DEV_SESSION);
    let child: ReturnType<typeof Bun.spawn> | undefined;
    let feed: { url: string; stop(): void } | undefined;
    const releaseLifecycleLock = await acquireLocalLifecycleLock(root);
    let shuttingDown = false;
    let restartTask: Promise<void> | undefined;
    let queueRestart = false;
    let timer: ReturnType<typeof setTimeout>;
    // Builds dist/, sets the local secrets, imports the pulled data on the first start with --reset, and starts the Kernel, which opens the folder as a release.
    const start = async (): Promise<string | undefined> => {
      if (shuttingDown) return undefined;
      try {
      for (const line of await setDevSecrets(root, kernel)) console.log(line);
      if (shuttingDown) return undefined;
      await refreshActionTypes(root);
      if (shuttingDown) return undefined;
      const { serverArtifact } = await buildLocal(root, { development: true });
      if (shuttingDown) return undefined;
      if (reset) {
        reset = false;
        const resetRecord = await resetData(root, kernel, pulled, option("confirm-schema"), { lockHeld: true });
        console.log(pulled
          ? `Replaced data/ with the data ${describePulled(pulled)}. Backup ${resetRecord.backup} holds the prior data and local queues. Reset ${resetRecord.executionId} used snapshot ${resetRecord.snapshotId} and retired ${resetRecord.callsRetired} calls and ${resetRecord.mailRetired} emails.`
          : `Emptied data/. Backup ${resetRecord.backup} holds the prior data and local queues. Reset ${resetRecord.executionId} retired ${resetRecord.callsRetired} calls and ${resetRecord.mailRetired} emails.`);
      }
      if (shuttingDown) return undefined;
      const started = Bun.spawn([kernel.binary], { env: kernel.env, stdout: "pipe", stderr: "pipe" });
      child = started;
      let output = "";
      void (async () => {
        const decoder = new TextDecoder();
        for await (const block of started.stdout as ReadableStream<Uint8Array>) output += decoder.decode(block);
      })();
      void (async () => {
        const decoder = new TextDecoder();
        for await (const block of started.stderr as ReadableStream<Uint8Array>) { const text = decoder.decode(block); output += text; process.stderr.write(text); }
      })();
      const wasmBundles = kernel.env.KONAE_FUNCTIONS_RUNNER_KIND === "wasm"
        ? Object.keys(serverArtifact?.bundles ?? {}).length
        : 0;
      // The Kernel allows 90 seconds each for preparation and native compilation.
      const startupDeadline = performance.now() + 15_000 + 180_000 * wasmBundles;
      while (performance.now() < startupDeadline && !/listening on/.test(output) && !shuttingDown) {
        if (started.exitCode !== null) break;
        await Bun.sleep(100);
      }
      if (shuttingDown) return undefined;
      if (!/listening on/.test(output)) {
        const message = started.exitCode === null ? "The Kernel took too long to start." : "The Kernel stopped before it served the app.";
        throw Error(`${message}\n${output.trim()}`);
      }
      const served = /Serving .+ at (\S+)/.exec(output)?.[1];
      if (!served) throw Error("The Kernel does not serve this folder. It said why above.");
      return served;
      } catch (error) {
        if (shuttingDown) return undefined;
        throw error;
      }
    };
    const stop = async () => { if (child && child.exitCode === null) { child.kill(); await child.exited; } };
    let resolveShutdown!: () => void;
    const shutdown = new Promise<void>((resolve) => { resolveShutdown = resolve; });
    let watcher: ReturnType<typeof watch> | undefined;
    let cleaned = false;
    const cleanup = async () => {
      if (cleaned) return;
      cleaned = true;
      shuttingDown = true;
      resolveShutdown();
      watcher?.close();
      clearTimeout(timer);
      await restartTask;
      await stop();
      feed?.stop();
      await rm(devSession, { force: true });
      await releaseLifecycleLock();
    };
    process.once("SIGINT", () => { shuttingDown = true; shutdownSignalCode = 130; process.exitCode = 130; resolveShutdown(); });
    process.once("SIGTERM", () => { shuttingDown = true; shutdownSignalCode = 143; process.exitCode = 143; resolveShutdown(); });
    try {
      const servedUrl = await start();
      if (!servedUrl || shuttingDown) return;
      console.log(servedUrl);
      // The events page answers on the next port, on this machine only. A taken port leaves the command, which reads the Kernel itself.
      const { serveEvents } = await import("./events");
      if (shuttingDown) return;
      const eventsPort = Number(option("events-port", String(Number(port) + 1)));
      try { feed = serveEvents(kernelRecord(kernel), settings.id, kernel.mailbox, eventsPort, testControls(kernel, () => localStateSummary(root))); }
      catch { console.warn(`Port ${eventsPort} is taken, so the events page is off. Choose another with --events-port, or run events.`); }
      if (shuttingDown) return;
      if (feed) await writeFile(devSession, JSON.stringify({ events: feed.url }), { mode: 0o600 });
      if (shuttingDown) return;
      const policy = settings.signupPolicy.mode;
      if (policy !== "open") {
        // The Kernel's own settings, so a command run by hand seals and addresses its links as the Kernel does.
        const envFile = path.join(root, ".konae/kernel.env");
        await writeFile(envFile, Object.entries(kernel.env).filter(([name]) => name.startsWith("KONAE_")).map(([name, value]) => `${name}=${value}\n`).join(""), { mode: 0o600 });
        const mailbox = kernel.mailbox;
        const test = await inviteTestAccount(kernel, mailbox).catch((error) => ({ failed: error instanceof Error ? error.message : String(error) }));
        if (test.line) console.log(test.line);
        if (test.failed) console.warn(`Could not invite the test account. ${test.failed}`);
        console.log(`The Signup Policy is ${policy}. To invite another address, run this, and open the link that lands in ${mailbox}:`);
        console.log(`  env $(cat ${envFile}) ${kernel.binary} invite <email> --app ${settings.id}`);
      }
      console.log(feed
        ? `Email, calls, scheduled runs and Signals of this app: ${feed.url} in a browser, or konae events --dir ${root} --json`
        : `See the email, calls, scheduled runs and Signals of this app with: konae events --dir ${root} --follow`);
      const restart = () => {
        if (shuttingDown) return;
        if (restartTask) { queueRestart = true; return; }
        restartTask = (async () => {
          do {
            queueRestart = false;
            if (shuttingDown) break;
            try {
              await stop();
              if (shuttingDown) break;
              const updated = await start();
              if (!shuttingDown && updated) console.log(`Updated ${updated}. Reload the app to see changes.`);
            } catch (error) {
              if (!shuttingDown) console.error(error instanceof Error ? error.message : error);
            }
          } while (queueRestart && !shuttingDown);
        })().finally(() => { restartTask = undefined; });
        return restartTask;
      };
      watcher = watch(appDir(root), { recursive: true }, (_, filename) => {
        if (shuttingDown || !filename || filename.startsWith(".") || filename.includes("node_modules") || !/\.(?:html|css|js|jsx|ts|tsx|json|sql)$/.test(filename)) return;
        clearTimeout(timer); timer = setTimeout(() => void restart(), 200);
      });
      await shutdown;
    } finally { await cleanup(); }
    return;
  }
}

if (import.meta.main) main().catch(error => {
  if (shutdownSignalCode !== undefined) process.exitCode = shutdownSignalCode;
  else {
    if (error instanceof ScenarioRefusal) console.error(JSON.stringify(error.result, null, 2));
    else console.error(error.message);
    process.exitCode = 1;
  }
});
