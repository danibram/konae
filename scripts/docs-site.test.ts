import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { slug } from "../examples/site/slug.js";
import { descriptionOf, renderMarkdown } from "../examples/site/render.js";
import { docsRoot, readSite, renderPage, sections, seedStatements } from "./docs-site";

const repo = path.resolve(import.meta.dir, "..");
const site = await readSite();
const template = readFileSync(path.join(repo, "examples/site/docs.template"), "utf8");
const htmlEscape = (value: string) => value.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));

function anchors(markdown: string) {
  const seen = new Map<string, number>();
  const ids = new Set<string>();
  let fenced = false;
  for (const line of markdown.split("\n")) {
    if (line.trimStart().startsWith("```")) fenced = !fenced;
    const heading = fenced ? null : /^#{1,6} (.+)$/.exec(line);
    if (heading) ids.add(slug(heading[1].replace(/[`*]/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1"), seen));
  }
  return ids;
}

function links(markdown: string) {
  const prose = markdown.replace(/^ *```[\s\S]*?^ *```/gm, "").replace(/`[^`\n]*`/g, "");
  return [...prose.matchAll(/\]\(([^)\s]+)\)/g)].map((match) => match[1]);
}

describe("the docs site", () => {
  test("the pieces page and AGENTS use the same module setup rows", () => {
    const needs = [
      "Another service calls this app", "This app calls another host", "Issue a private PDF",
      "Give an external MCP client read access to app rows", "Accept file versions from a device or test fixture",
      "Receive files over FTPS",
    ];
    const rows = (file: string) => readFileSync(path.join(repo, file), "utf8").split("\n")
      .filter((line) => needs.some((need) => line.startsWith(`| ${need} |`)));
    expect(rows("docs/pieces.md")).toHaveLength(needs.length);
    expect(rows("docs/pieces.md")).toEqual(rows("AGENTS.md"));
  });

  test("the routes reference explains module choices for routes, calls and API keys", () => {
    const reference = readFileSync(path.join(docsRoot, "reference/routes-and-calls.md"), "utf8");
    expect(reference).toContain("Turn on `http` with `module_set` before a route runs");
    expect(reference).toContain("Turn on `calls` with `module_set` before an outgoing call runs");
    expect(reference).toContain("An action scope needs `actions` and `http`. A files-only key needs `files`");
  });

  test("generated pages ship their body and a description from the first paragraph", () => {
    for (const page of site.pages) {
      const html = renderPage(template, site, page, "test");
      const firstParagraph = descriptionOf(page.markdown);
      expect(html).toContain("<article id=\"content\"");
      expect(firstParagraph.length).toBeLessThanOrEqual(160);
      expect(html).toContain(`<meta name="description" content="${htmlEscape(firstParagraph)}">`);
      expect(html).toContain(`<title>${htmlEscape(page.title)} · Konae docs</title>`);
      expect(html).toContain(renderMarkdown(page.markdown));
      expect(html).not.toContain("Loading the markdown of this page");
    }
  });

  test("each docs app card image exists and stays under the size limit", () => {
    const image = readFileSync(path.join(repo, "examples/site/public/docs/card.png"));
    expect(image.byteLength).toBeLessThan(600 * 1024);
    expect(image.readUInt32BE(16)).toBeGreaterThanOrEqual(300);
  });

  test("every page in the navigation exists, has a title and its own route", () => {
    expect(site.pages.length).toBeGreaterThan(30);
    expect(new Set(site.pages.map((page) => page.route)).size).toBe(site.pages.length);
    for (const page of site.pages) expect(["mcp", "oauth", "_konae", "runtime"]).not.toContain(page.route.split("/")[0]);
  });

  test("every relative link names a file that exists and a heading it holds", () => {
    const broken: string[] = [];
    for (const page of site.pages) {
      for (const link of links(page.markdown)) {
        if (/^[a-z][a-z0-9+.-]*:/i.test(link)) continue;
        const [file, anchor] = link.split("#");
        const target = file ? path.resolve(docsRoot, path.dirname(page.source), file) : path.join(docsRoot, page.source);
        if (!existsSync(target)) { broken.push(`docs/${page.source}: ${link} names no file`); continue; }
        if (anchor && target.endsWith(".md") && !anchors(readFileSync(target, "utf8")).has(anchor)) broken.push(`docs/${page.source}: ${link} names no heading`);
      }
    }
    expect(broken).toEqual([]);
  });

  test("the repository's own README and the docs app's README link to files that exist", () => {
    const broken: string[] = [];
    for (const source of ["README.md", "examples/site/README.md"]) {
      for (const link of links(readFileSync(path.join(repo, source), "utf8"))) {
        if (/^[a-z][a-z0-9+.-]*:/i.test(link) || link.startsWith("#")) continue;
        const [file, anchor] = link.split("#");
        const target = path.resolve(repo, path.dirname(source), file);
        if (!existsSync(target)) broken.push(`${source}: ${link} names no file`);
        else if (anchor && !anchors(readFileSync(target, "utf8")).has(anchor)) broken.push(`${source}: ${link} names no heading`);
      }
    }
    expect(broken).toEqual([]);
  });

  test("the MCP reference names every tool the Kernel registers", () => {
    const registered = new Set<string>();
    for (const crate of ["kernel", "konae"]) {
      const dir = path.join(repo, "kernel-rs", crate, "src");
      for (const file of readdirSync(dir, { recursive: true }).filter((name) => name.endsWith(".rs"))) {
        for (const match of readFileSync(path.join(dir, file), "utf8").matchAll(/\btool\(\s*"([a-z_]+)"/g)) registered.add(match[1]);
      }
    }
    expect(registered.size).toBeGreaterThan(60);
    const reference = readFileSync(path.join(docsRoot, "mcp-tools.md"), "utf8");
    expect([...registered].filter((name) => !reference.includes(`\`${name}\``))).toEqual([]);
  });

  test("the search rows open at headings and fit one owner SQL script each", () => {
    const rows = site.pages.flatMap(sections);
    expect(new Set(rows.map((row) => row.path)).size).toBe(rows.length);
    const capsule = site.pages.find((page) => page.source === "capsule.md")!;
    expect(sections(capsule).map((row) => row.path)).toContain("capsule#layout");
    for (const statement of seedStatements(site)) expect(Buffer.byteLength(statement)).toBeLessThan(100_000);
  });

  test("a diagram is a mermaid block with a paragraph under it, and search reads the paragraph only", () => {
    const withDiagrams = site.pages.filter((page) => page.markdown.includes("```mermaid"));
    expect(withDiagrams.length).toBeGreaterThan(5);
    for (const page of withDiagrams) {
      for (const after of page.markdown.split(/^```mermaid\n[\s\S]*?^```\n/m).slice(1)) {
        expect(after.trimStart(), `docs/${page.source} has a diagram with no paragraph under it`).toMatch(/^[A-Z`]/);
      }
      for (const block of page.markdown.match(/^```mermaid\n[\s\S]*?^```\n/gm) ?? []) {
        expect(block.replace(/<br\/>/g, ""), `docs/${page.source}`).not.toMatch(/<[a-z]/i);
      }
      const text = sections(page).map((row) => row.body).join(" ");
      expect(text, `docs/${page.source}`).not.toMatch(/flowchart (TB|LR)|sequenceDiagram|participant \w+ as/);
    }
  });
});
