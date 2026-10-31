// Runs docs/mcp-walkthrough.md against real Kernels and records every
// command, tool call and HTTP request with its answer. The integration test
// renders the record into the document's marked blocks and fails when the
// document differs, so the walkthrough cannot drift from what the Kernel says.
import path from "node:path";

const repo = path.resolve(import.meta.dir, "..");
const kernelBinary = path.join(repo, "kernel-rs/target/debug/konae-kernel");
const PROTOCOL = "2025-11-25";

/** The URLs the document shows, whatever ports the run used. */
export const SHOWN = { a: "http://localhost:4173", b: "http://localhost:4174", dev: "http://localhost:8796" };

type Block = { lang: string; text: string };

/**
 * Collects the blocks of each marked step and replaces every value that
 * changes between runs (IDs, tokens, hashes, times, ports) with a named
 * placeholder, so two runs render the same document.
 */
export class Recorder {
  readonly steps = new Map<string, Block[]>();
  private readonly names: [string, string][] = [];
  current = "";

  step(id: string) { this.current = id; if (!this.steps.has(id)) this.steps.set(id, []); }
  /** Shows `value` as `placeholder` wherever it appears in a rendered block, also URL-encoded. */
  name(value: string | undefined | null, placeholder: string) {
    if (!value || this.names.some(([known]) => known === value)) return;
    const shown = (text: string) => placeholder.startsWith("<") ? placeholder : text;
    const once = encodeURIComponent(value);
    this.names.push([value, placeholder]);
    if (once !== value) this.names.push([once, shown(encodeURIComponent(placeholder))]);
    if (encodeURIComponent(once) !== once) this.names.push([encodeURIComponent(once), shown(encodeURIComponent(encodeURIComponent(placeholder)))]);
    this.names.sort((a, b) => b[0].length - a[0].length);
  }
  normalize(text: string) {
    for (const [value, placeholder] of this.names) text = text.split(value).join(placeholder);
    return text
      .replace(/\b(?:lb|lbk)_[A-Za-z0-9_]{20,}/g, (token) => token.startsWith("lbk_") ? "<api key>" : "<token>")
      .replace(/\b[0-9a-f]{64}\b/g, "<sha256>")
      .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z/g, "<time>")
      .replace(/\b1\d{12}\b/g, "<time ms>")
      .replace(/\bt=\d{10}\b/g, "t=<unix seconds>")
      // The Kernel writes the parameters of returnTo in no fixed order.
      .replace(/returnTo=%2Foauth%2Fauthorize%3F[^\n]+/g, "returnTo=%2Foauth%2Fauthorize%3F<the same parameters, URL-encoded>")
      .replace(/"(at|createdAt|expiresAt|updatedAt|verifiedAt|acceptedAt|revokedAt|confirmedAt|retainUntil|pausedAt|cancelledAt|t)":\s*\d{9,10}\b/g, '"$1": <unix seconds>');
  }
  add(lang: string, text: string) { this.steps.get(this.current)!.push({ lang, text }); }
  json(value: unknown) { this.add("json", JSON.stringify(value, null, 2)); }
  render(id: string) {
    const blocks = this.steps.get(id);
    if (!blocks) throw Error(`The walkthrough recorded no step ${id}.`);
    return blocks.map((block) => `\`\`\`${block.lang}\n${this.normalize(block.text).trimEnd()}\n\`\`\``).join("\n\n");
  }
}

/** Replaces the content between `<!-- walkthrough:id -->` and `<!-- /walkthrough:id -->` with the recorded blocks. */
export function renderDocument(document: string, recorder: Recorder) {
  const seen = new Set<string>();
  const out = document.replace(/<!-- walkthrough:([a-z0-9-]+) -->\n[\s\S]*?<!-- \/walkthrough:\1 -->/g, (_, id: string) => {
    seen.add(id);
    return `<!-- walkthrough:${id} -->\n${recorder.render(id)}\n<!-- /walkthrough:${id} -->`;
  });
  const missing = [...recorder.steps.keys()].filter((id) => !seen.has(id));
  if (missing.length) throw Error(`docs/mcp-walkthrough.md has no block for ${missing.join(", ")}.`);
  return out;
}

export type ToolAnswer = { result?: unknown; refused?: string; error?: string };

/** One tools/call, answered as the Kernel answers it: a result, a tool error, or a JSON-RPC error. */
export async function callTool(endpoint: string, headers: Record<string, string>, name: string, args: object = {}): Promise<ToolAnswer> {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json, text/event-stream", "MCP-Protocol-Version": PROTOCOL, ...headers },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
  });
  if (!response.ok) return { error: `HTTP ${response.status} ${await response.text()}` };
  const body = await response.json() as { error?: { message: string }; result?: { isError?: boolean; content?: { text: string }[]; structuredContent?: { result: unknown } } };
  if (body.error) return { error: body.error.message };
  if (body.result?.isError) return { refused: body.result.content?.[0]?.text ?? "" };
  return { result: body.result?.structuredContent?.result };
}

export type KernelProcess = { url: string; data: string; port: number; child: ReturnType<typeof Bun.spawn> };

function cleanEnv() {
  return Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))) as Record<string, string>;
}

export async function startKernel(port: number, data: string, setupKey: string): Promise<KernelProcess> {
  const url = `http://127.0.0.1:${port}`;
  const child = Bun.spawn([kernelBinary], {
    env: { ...cleanEnv(), KONAE_DATA_DIR: data, KONAE_KERNEL_PORT: String(port), KONAE_BIND: "127.0.0.1", KONAE_ADMIN_KEY: setupKey, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_BUN_PATH: process.execPath, KONAE_MAIL_TRANSPORT: "development" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 150; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) return { url, data, port, child }; } catch {}
    if (child.exitCode !== null) throw Error(`The Kernel on port ${port} stopped during startup.`);
    await Bun.sleep(100);
  }
  throw Error(`The Kernel on port ${port} did not start.`);
}

/** Runs the CLI as `bun run konae -- <args>` would, and returns what it printed. */
export async function cli(args: string[], env: Record<string, string> = {}) {
  const child = Bun.spawn([process.execPath, path.join(repo, "scripts/konae.ts"), ...args], { cwd: repo, env: { ...cleanEnv(), ...env }, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  return { code, output: (stdout + stderr).trimEnd() };
}
