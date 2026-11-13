#!/usr/bin/env bun
// Builds the docs site's content from docs/. The markdown under docs/ is the
// only source: this script copies each page into the docs app's Capsule
// (examples/docs-site), writes one HTML shell per page, llms.txt and the rows
// the search reads. Everything it writes is ignored by git.
//
//   bun run docs:site          writes the files
//   bun run docs:site --seed   also loads the search rows into the published app
//   --dir <folder>             writes into a copy of the app instead of examples/docs-site
import { copyFile, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { slug } from "../examples/docs-site/slug.js";
import { McpClient } from "../src/mcp/client";

const repo = path.resolve(import.meta.dir, "..");
export const docsRoot = path.join(repo, "docs");
const dirFlag = process.argv.indexOf("--dir");
/** The docs app's Capsule. --dir names a copy of it, such as one published to a test Kernel. */
export const appRoot = dirFlag > 0 ? path.resolve(process.argv[dirFlag + 1]) : path.join(repo, "examples/docs-site");

type NavEntry = string | { page: string; label: string };
type SiteConfig = { title: string; summary: string; repository: string; nav: { section: string; pages: NavEntry[] }[]; include: string[]; routes?: Record<string, string> };
export type Page = { route: string; source: string; title: string; label: string; markdown: string };
export type Site = { config: SiteConfig; pages: Page[] };

/**
 * docs/README.md is the home, and every other file keeps its path without
 * `.md`, unless site.json names another route for it. docs/mcp.md needs one:
 * every Deployment answers /mcp with its runtime endpoint.
 */
export const routeOf = (source: string, routes: Record<string, string> = {}) =>
  routes[source] ?? (source === "README.md" ? "index" : source.replace(/(^|\/)README\.md$/, "$1index").replace(/\.md$/, ""));

async function markdownUnder(folder: string): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(path.join(docsRoot, folder), { withFileTypes: true })) {
    const relative = path.posix.join(folder, entry.name);
    if (entry.isDirectory()) found.push(...await markdownUnder(relative));
    else if (entry.name.endsWith(".md")) found.push(relative);
  }
  return found.sort();
}

/** The pages site.json names in its nav, then the folders it includes without listing each file. */
export async function readSite(): Promise<Site> {
  const config = JSON.parse(await readFile(path.join(docsRoot, "site.json"), "utf8")) as SiteConfig;
  const labels = new Map<string, string | undefined>();
  for (const { pages } of config.nav) for (const entry of pages) labels.set(typeof entry === "string" ? entry : entry.page, typeof entry === "string" ? undefined : entry.label);
  for (const folder of config.include) for (const source of await markdownUnder(folder)) if (!labels.has(source)) labels.set(source, undefined);
  const pages: Page[] = [];
  for (const [source, label] of labels) {
    const markdown = await readFile(path.join(docsRoot, source), "utf8");
    const title = /^# (.+)$/m.exec(markdown)?.[1].replace(/`/g, "");
    if (!title) throw Error(`docs/${source} has no "# " title.`);
    pages.push({ route: routeOf(source, config.routes), source, title, label: label ?? title, markdown });
  }
  return { config, pages };
}

const esc = (value: string) => value.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
/** The way from a page back to the site's root: empty at the top, `../` one folder down. */
const rootOf = (route: string) => "../".repeat(route.split("/").length - 1);
const hrefOf = (root: string, route: string) => (route === "index" ? root || "./" : root + route);

function shell(template: string, site: Site, page: Pick<Page, "route" | "title">) {
  const root = rootOf(page.route);
  const bySource = new Map(site.pages.map((entry) => [entry.source, entry]));
  const nav = site.config.nav.map(({ section, pages }) => {
    const items = pages.map((entry) => {
      const target = bySource.get(typeof entry === "string" ? entry : entry.page)!;
      const current = target.route === page.route ? ' aria-current="page"' : "";
      return `<li><a href="${hrefOf(root, target.route)}"${current}>${esc(target.label)}</a></li>`;
    });
    return `      <h2>${esc(section)}</h2>\n      <ul>${items.join("")}</ul>`;
  }).join("\n");
  const values: Record<string, string> = { title: esc(page.title), root, route: page.route, home: hrefOf(root, "index"), nav };
  return template.replace(/\{\{(\w+)\}\}/g, (_, key) => values[key]);
}

/** Markdown as the words a reader sees, for the search rows. */
/**
 * Search text for one section. Comments, list markers and table rules go; a
 * table row keeps its cells apart as "first: second, third." so a snippet
 * still reads as a row.
 */
function plain(markdown: string) {
  return markdown
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/^```.*$/gm, "")
    .replace(/^\s*\|.*\|\s*\n\s*\|?[\s:|-]+\|[\s:|-]*$/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s*\|?[\s:|-]+\|[\s:|-]*$/gm, "")
    .replace(/^\s*\|(.*)\|\s*$/gm, (_, row: string) => {
      const [first, ...rest] = row.split("|").map((cell) => cell.trim()).filter(Boolean);
      return rest.length ? `${first}: ${rest.map((cell) => cell.replace(/[.:;,]$/, "")).join(". ")}.` : (first ?? "");
    })
    .replace(/^\s*(?:[-*+]|\d+\.)\s+/gm, "")
    .replace(/`/g, "")
    .replace(/[*|]|^#+ |^ *> ?/gm, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/** One row per `## ` section, so a result opens at the heading that holds the match. */
export function sections(page: Page) {
  const rows: { path: string; page: string; title: string; body: string }[] = [];
  const seen = new Map<string, number>();
  let fenced = false;
  let diagram = false;
  let current = { path: page.route, title: page.title, lines: [] as string[] };
  const close = () => { const body = plain(current.lines.join("\n")); if (body) rows.push({ path: current.path, page: page.title, title: current.title, body }); };
  for (const line of page.markdown.split("\n")) {
    if (line.trimStart().startsWith("```")) { fenced = !fenced; diagram = fenced && line.trim() === "```mermaid"; }
    // A diagram's source is drawn, not read, so search skips it. The paragraph under it says the same in words.
    if (diagram) continue;
    const heading = fenced ? null : /^(#{1,6}) (.+)$/.exec(line);
    if (heading) {
      const id = slug(heading[2].replace(/[`*]/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"), seen);
      if (heading[1] === "##") { close(); current = { path: `${page.route}#${id}`, title: plain(heading[2]), lines: [] }; continue; }
    }
    if (!/^# /.test(line) || fenced) current.lines.push(line);
  }
  close();
  return rows;
}

const quote = (value: string) => `'${value.replace(/'/g, "''")}'`;

/**
 * The statements that replace every search row. The Kernel refuses an owner
 * SQL script over 100,000 bytes, so each statement stays under 80,000.
 */
export function seedStatements(site: Site) {
  const statements = ["DELETE FROM sections"];
  let batch: string[] = [];
  let size = 0;
  const flush = () => {
    if (batch.length) statements.push(`INSERT OR REPLACE INTO sections (path, page, title, body) VALUES\n${batch.join(",\n")}`);
    batch = [];
    size = 0;
  };
  for (const row of site.pages.flatMap(sections)) {
    const values = `(${[row.path, row.page, row.title, row.body.slice(0, 20_000)].map(quote).join(", ")})`;
    if (size + Buffer.byteLength(values) > 80_000) flush();
    batch.push(values);
    size += Buffer.byteLength(values);
  }
  flush();
  return statements;
}

function llms(site: Site) {
  const bySource = new Map(site.pages.map((entry) => [entry.source, entry]));
  const lines = [`# ${site.config.title}`, "", `> ${site.config.summary}`, ""];
  for (const { section, pages } of site.config.nav) {
    lines.push(`## ${section}`);
    for (const entry of pages) {
      const page = bySource.get(typeof entry === "string" ? entry : entry.page)!;
      const lead = plain(/^# .+\n+([^\n#|`-][^\n]*)/m.exec(page.markdown)?.[1] ?? "");
      lines.push(`- [${page.label}](content/${page.route}.md)${lead ? `: ${lead.split(/(?<=\.)\s/)[0]}` : ""}`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

export async function writeSite(site: Site) {
  const template = await readFile(path.join(appRoot, "page.template"), "utf8");
  for (const generated of ["pages", "public/content", "public/vendor", "index.html", "public/llms.txt", "seed.sql"]) await rm(path.join(appRoot, generated), { recursive: true, force: true });
  const write = async (relative: string, contents: string) => {
    await mkdir(path.dirname(path.join(appRoot, relative)), { recursive: true });
    await writeFile(path.join(appRoot, relative), contents);
  };
  for (const page of site.pages) {
    await write(`public/content/${page.route}.md`, page.markdown);
    await write(page.route === "index" ? "index.html" : `pages/${page.route}.html`, shell(template, site, page));
  }
  await write("public/content/site.json", JSON.stringify({
    repository: site.config.repository.replace(/\/$/, ""),
    pages: Object.fromEntries(site.pages.map((page) => [page.route, { title: page.title, source: page.source }])),
    routes: Object.fromEntries(site.pages.map((page) => [page.source, page.route])),
  }, null, 2));
  await write("public/llms.txt", llms(site));
  // shell.js draws a page's mermaid blocks with this file and loads it only on a page that has one.
  if (site.pages.some((page) => /^```mermaid$/m.test(page.markdown))) {
    const renderer = path.join(appRoot, "node_modules/@mermaid-js/tiny/dist/mermaid.tiny.js");
    await mkdir(path.join(appRoot, "public/vendor"), { recursive: true });
    await copyFile(renderer, path.join(appRoot, "public/vendor/mermaid.js")).catch(() => {
      throw Error(`The docs hold a diagram and ${path.relative(repo, renderer)} is missing. Run bun install in examples/docs-site.`);
    });
  }
  await write("seed.sql", `-- The search rows, one per section. bun run docs:site --seed loads them.\n${seedStatements(site).join(";\n")};\n`);
}

/** Runs each statement on the published app from the app folder `folder`, with the credential `konae login` saved there. */
async function seed(site: Site, folder: string) {
  const missing = () => { throw Error(`The docs are not published from ${folder} yet. examples/docs-site/README.md has the commands.`); };
  // konae publish saves the Deployment it wrote, and database_execute names a Deployment.
  const deployment = JSON.parse(await readFile(path.join(folder, ".konae/published.json"), "utf8").catch(missing));
  const saved = JSON.parse(await readFile(path.join(folder, ".konae/credentials.json"), "utf8").catch(missing)) as { url: string; token: string };
  const token = process.env.KONAE_MCP_TOKEN ?? saved.token;
  const client = new McpClient(`${saved.url.replace(/\/$/, "")}/mcp`, { token });
  const call = (name: string, args: object) => client.call<{ rows?: unknown[] }>(name, args);
  for (const sql of seedStatements(site)) await call("database_execute", { deploymentId: deployment.id, operation: "sql", input: { sql } });
  const count = await call("database_execute", { deploymentId: deployment.id, operation: "sql", input: { sql: "SELECT count(*) AS sections FROM sections LIMIT 1" } });
  console.log("Search rows on the Deployment:", JSON.stringify(count.rows ?? count));
}

if (import.meta.main) {
  const site = await readSite();
  await writeSite(site);
  console.log(`Wrote ${site.pages.length} pages into ${path.relative(repo, appRoot) || appRoot}.`);
  const seedAt = process.argv.indexOf("--seed");
  if (seedAt >= 0) {
    const folder = process.argv[seedAt + 1];
    if (!folder) throw Error("Name the app folder the docs publish from: bun run docs:site --seed /tmp/konae-docs");
    await seed(site, path.resolve(folder));
  }
}
