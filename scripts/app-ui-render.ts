// Renders the design system into the stylesheet a published app carries.
// A Capsule has no bare imports and its first message holds every file, so the
// bundle keeps only the pieces a small app screen uses, in one variant each.
// src/lib/templates/app-ui.test.ts fails when the checked-in file differs from this output.
import { readFileSync } from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dir, "..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");

/** The components an app screen can use. Charts, menus, tabs, dialogs and toasts stay in the editor. */
const kept = /^\.(ui-(button|input|select|textarea|field|badge|card|empty|skeleton|loading|alert|check|radio|avatar|progress|table|stats|spark|disclosure|list|composer|form|actions|skip|bar|steps|pre|tile|tiles|page|eyebrow|title|lede|identity|status|section-head|code|kbd|tick)\b)/;
const keptKeyframes = new Set(["ui-skeleton-shimmer", "ui-loading-in"]);

type Block = { head: string; body: string | null; text: string };

/** Splits CSS into its top-level rules, at-rules and comments. */
function blocks(css: string): Block[] {
  const out: Block[] = [];
  let index = 0;
  while (index < css.length) {
    while (index < css.length && /\s/.test(css[index])) index++;
    if (index >= css.length) break;
    if (css.startsWith("/*", index)) {
      const end = css.indexOf("*/", index) + 2;
      out.push({ head: "", body: null, text: css.slice(index, end) });
      index = end;
      continue;
    }
    const open = css.indexOf("{", index);
    if (open < 0) break;
    let depth = 1;
    let cursor = open + 1;
    while (depth > 0 && cursor < css.length) {
      if (css[cursor] === "{") depth++;
      else if (css[cursor] === "}") depth--;
      cursor++;
    }
    out.push({ head: css.slice(index, open).trim(), body: css.slice(open + 1, cursor - 1), text: css.slice(index, cursor) });
    index = cursor;
  }
  return out;
}

const target = (selector: string) => selector.trim().replace(/^(:root|\[data-ds="clean"\])\s*/, "");

function keep(selector: string): boolean {
  return selector.split(",").some((part) => {
    const rest = target(part);
    return rest === "" || kept.test(rest) || /^(button, input, select, textarea|\*)/.test(part.trim());
  });
}

function filter(css: string): string {
  const out: string[] = [];
  for (const block of blocks(css)) {
    if (block.body === null) continue;
    if (block.head.startsWith("@media")) {
      const inner = filter(block.body);
      if (inner) out.push(`${block.head} {\n${inner.split("\n").map((line) => `  ${line}`).join("\n")}\n}`);
    } else if (block.head.startsWith("@keyframes")) {
      if (keptKeyframes.has(block.head.split(/\s+/)[1])) out.push(block.text);
    } else if (block.head.startsWith("@")) {
      continue;
    } else if (keep(block.head)) {
      out.push(block.text);
    }
  }
  return out.join("\n");
}

function section(css: string, from: string, to?: string) {
  const start = css.indexOf(from);
  if (start < 0) throw new Error(`design system CSS lost the marker ${from}`);
  const end = to ? css.indexOf(to, start) : css.length;
  if (end < 0) throw new Error(`design system CSS lost the marker ${to}`);
  return css.slice(start, end);
}

const base = `*, *::before, *::after { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; min-height: 100vh; background: var(--ds-canvas); color: var(--ds-text); }
h1, h2, h3, p { margin-top: 0; }
img { max-width: 100%; }
a { color: inherit; text-underline-offset: 3px; }
[hidden] { display: none !important; }
.sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }
.ui-button:focus-visible, .ui-input:focus-visible, a:focus-visible { outline: 2px solid var(--ds-blue); outline-offset: 2px; }`;

export function renderBundles() {
  const system = read("src/design-system.css");
  const ui = read("src/components/ui/ui.css");
  const clean = read("src/components/ui/variant-clean.css");
  const common = [
    filter(section(system, ":root {", "/* Shared ui + legacy chrome class names */")),
    filter(section(system, "/* Loading: skeleton", ".boot-gap")),
    filter(ui),
  ].join("\n");
  const cleanOnly = filter([section(clean, "[data-ds=\"clean\"] {", "/* Workspace chrome"), section(clean, "/* App patterns */")].join("\n").replaceAll('[data-ds="clean"]', ":root"));
  const brutalFonts = ":root { --ds-font: \"Bricolage Grotesque\", ui-sans-serif, system-ui, sans-serif; --ds-mono: \"JetBrains Mono\", ui-monospace, \"SF Mono\", Menlo, monospace; }";
  const header = (fonts: string) => `@import url('https://fonts.googleapis.com/css2?${fonts}&display=swap');\n/* Konae design system. Change a token on :root to restyle the app. */\n`;
  return {
    brutal: `${header("family=Bricolage+Grotesque:opsz,wght@12..96,400..800&family=JetBrains+Mono:wght@400..700")}${common}\n${brutalFonts}\n${base}\n`,
    clean: `${header("family=Inter:wght@400..700")}${common}\n${cleanOnly}\n${base}\n`,
  };
}

export function renderAppUi(): string {
  const { brutal, clean } = renderBundles();
  return `// Written by scripts/render-app-ui.ts from src/design-system.css and src/components/ui. Do not edit by hand.\n/** The design system as one stylesheet per variant, ready to be a Capsule's styles.css. */\nexport const appUiStyles = {\n  brutal: ${JSON.stringify(brutal)},\n  clean: ${JSON.stringify(clean)},\n} as const;\n\nexport type AppUiVariant = keyof typeof appUiStyles;\n`;
}
