import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

// CONTEXT.md says how each term introduced in November 2026 is written in a
// sentence. This test holds the docs, the editor's words and the Kernel's
// messages to that list, so a term keeps one spelling wherever a person reads it.

const root = path.resolve(import.meta.dir, "../..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");
const context = read("CONTEXT.md");

function lowercaseTerms(): string[] {
  const sentence = context.match(/are written in lowercase, because[^:]*: ([^.]+)\./);
  if (!sentence) throw new Error("CONTEXT.md no longer states which terms are written in lowercase.");
  return sentence[1].split(/, (?:and )?| and /).map((term) => term.trim());
}

const headings = new Set([...context.matchAll(/^\*\*([^*]+)\*\*:$/gm)].map((match) => match[1].toLowerCase()));

/** "API key" becomes "API Key", "signing secret" becomes "Signing Secret". */
const titled = (term: string) => term.replace(/\b[a-z]/g, (letter) => letter.toUpperCase());

function files(dir: string, keep: (file: string) => boolean): string[] {
  return readdirSync(path.join(root, dir)).flatMap((name) => {
    const relative = path.join(dir, name);
    if (statSync(path.join(root, relative)).isDirectory()) return files(relative, keep);
    return keep(relative) ? [relative] : [];
  });
}

const prose = [
  "README.md",
  "docs/architecture.md",
  "docs/mcp.md",
  "docs/sdk.md",
  "docs/README.md",
  "docs/operate.md",
  "docs/concepts.md",
  "docs/capsule.md",
  "docs/server-actions.md",
  "docs/gate-and-kernel.md",
  "docs/accounts-and-access.md",
  "docs/cli.md",
  "docs/mcp-tools.md",
  "docs/pieces.md",
  "docs/limits.md",
  "docs/divergences.md",
  "docs/examples.md",
  ...files("docs/guides", (file) => file.endsWith(".md")),
  "docs/research/showcase-findings.md",
  "docs/plans/platform-launch.md",
  "docs/plans/self-use-and-sdk-modules.md",
  ...files("docs/adr", (file) => file.endsWith(".md")),
];

const code = [
  "src/sdk/contract.ts",
  "src/kernel/actions.ts",
  "src/agent/system-prompt.ts",
  "src/lib/action-contract.ts",
  "src/lib/action-conformance.ts",
  ...files("src/components", (file) => /\.tsx?$/.test(file) && !file.includes(".test.")),
  ...["kernel-rs/kernel/src", "kernel-rs/konae/src"].flatMap((dir) => files(dir, (file) => file.endsWith(".rs") || file.endsWith("-cases.json") || file.endsWith(".html"))),
];

/** The text a person reads: CONTEXT.md without its headings, whole Markdown files, and string literals in code. */
function readable(file: string): string {
  // A Rust file's tests name fixtures, such as an app called "Invitation test", that nobody reads.
  const text = file.endsWith(".rs") ? read(file).split("#[cfg(test)]")[0] : read(file);
  if (file === "CONTEXT.md") return text.split("\n").filter((line) => !/^\*\*[^*]+\*\*:$/.test(line)).join("\n");
  if (file.endsWith(".md")) return text;
  return [...text.matchAll(/"(?:[^"\\\n]|\\.)*"|`(?:[^`\\]|\\.)*`/g)].map((match) => match[0]).join("\n");
}

/** A capitalized term after a lowercase word or a comma is the term written with the wrong case. */
function capitalizedUses(file: string, term: string): string[] {
  const wrong = titled(term);
  if (wrong === term) return [];
  const pattern = new RegExp(`(?<=[a-z,;(] )${wrong.replace(/ /g, " ")}s?\\b(?! only)`, "g");
  return [...readable(file).matchAll(pattern)].map((match) => `${file}: ${match[0]}`);
}

describe("the words of CONTEXT.md", () => {
  test("every term written in lowercase has its own entry", () => {
    const terms = lowercaseTerms();
    expect(terms.length).toBeGreaterThan(10);
    expect(terms.filter((term) => !headings.has(term.toLowerCase()))).toEqual([]);
  });

  test("the terms introduced with the HTTP module, copies, maintenance, accounts and mail each have one entry", () => {
    const introduced = [
      "API Key", "Signing Secret", "Guard", "Callers", "Export", "Copy", "Maintenance", "Revocation Record",
      "Invitation", "Invitation Code", "Email Job", "Sender Preflight", "Action Contract", "SDK Module", "Usage", "Signal",
    ];
    const entries = [...context.matchAll(/^\*\*([^*]+)\*\*:$/gm)].map((match) => match[1]);
    for (const term of introduced) expect(entries.filter((entry) => entry === term)).toEqual([term]);
  });

  test("docs, the editor and the Kernel write those terms the way CONTEXT.md does", () => {
    const terms = lowercaseTerms();
    const found = ["CONTEXT.md", ...prose, ...code].flatMap((file) => terms.flatMap((term) => capitalizedUses(file, term)));
    expect(found).toEqual([]);
  });

  test("the check finds a capitalized term in the middle of a sentence and leaves a policy name alone", () => {
    const sample = "A key is an API Key. The app is Invitation only, and an Invitation is sent.";
    const pattern = (term: string) => new RegExp(`(?<=[a-z,;(] )${titled(term)}s?\\b(?! only)`, "g");
    expect(sample.match(pattern("API key"))).toEqual(["API Key"]);
    expect(sample.match(pattern("invitation"))).toEqual(["Invitation"]);
  });
});
