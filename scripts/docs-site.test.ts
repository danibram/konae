import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { slug } from "../examples/docs-site/slug.js";
import { docsRoot, readSite, sections, seedStatements } from "./docs-site";

const repo = path.resolve(import.meta.dir, "..");
const site = await readSite();

/** The ids a page's headings get, by the rule the site and GitHub share. */
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

/** Every markdown link outside code, as written. */
function links(markdown: string) {
  const prose = markdown.replace(/^ *```[\s\S]*?^ *```/gm, "").replace(/`[^`\n]*`/g, "");
  return [...prose.matchAll(/\]\(([^)\s]+)\)/g)].map((match) => match[1]);
}

describe("the docs site", () => {
  test("every page in the navigation exists, has a title and its own route", () => {
    expect(site.pages.length).toBeGreaterThan(30);
    expect(new Set(site.pages.map((page) => page.route)).size).toBe(site.pages.length);
    // The Kernel keeps these paths on every Deployment.
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
    for (const source of ["README.md", "examples/docs-site/README.md"]) {
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
      for (const file of readdirSync(dir).filter((name) => name.endsWith(".rs"))) {
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
});
