// The static publishing contract: which Capsule files become a Build Artifact
// and at which path each one is served. The CLI (`littlebox build`), the
// editor's browser build and the exported kit's scripts/build.mjs all run this
// file, so it is plain JavaScript with no imports. Each door supplies how to
// read a file and how to bundle a module; the rules below are the same for all.
//
// Pages are every .html at the Capsule root and every .html under pages/, which
// is served without its pages/ prefix. Files under public/ are served as they
// are, without the public/ prefix. Every other file ships only when a page, or
// a stylesheet a page loads, references it. A page that is a fragment runs in
// the Capsule document with styles.css and the bundled app inline. A page that
// is a full document keeps its own head and loads the SDK from /runtime.js.

/** The attribute that marks the SDK script, so tools that rebuild a document can find it. */
export const runtimeScriptAttribute = "data-littlebox-runtime";

/** The SDK file a document page loads. The build adds it; a Capsule cannot hold its own. */
export const runtimeFile = "runtime.js";

export const routingModes = ["pages", "spa"];
export const routingMessage = 'Routing must be "pages" or "spa".';

const reservedRoots = ["_littlebox", "oauth", ".well-known", "mcp"];
const fetchingRels = ["stylesheet", "icon", "apple-touch-icon", "mask-icon", "manifest", "preload", "modulepreload", "prefetch"];
const defaultBody = "<main id=app></main>";

/**
 * Refuses a routing value other than "pages" or "spa". Undefined means "pages".
 * The Kernel refuses the same values with the same text at release_plan.
 */
export function checkRouting(value) {
  if (value === undefined || value === null) return "pages";
  if (!routingModes.includes(value)) throw new Error(routingMessage);
  return value;
}

/** The served path of a Capsule file, or null when the file is not a page or a public file. */
export function servedPath(source) {
  if (source.startsWith("public/")) return source.slice("public/".length) || null;
  if (!source.endsWith(".html")) return null;
  if (!source.includes("/")) return source;
  if (source.startsWith("pages/")) return source.slice("pages/".length);
  return null;
}

/** True when the HTML is a whole document, false when it is a fragment for the Capsule body. */
export function isDocument(html) {
  return /^\s*(?:<!--[\s\S]*?-->\s*)*<(?:!doctype|html)[\s>]/i.test(html);
}

/**
 * The one document shape a fragment page runs in, shared by the preview and
 * every Deployment. The compiled module is emitted as is: esbuild output may
 * end in a top-level `export`, which is only legal at module scope, and a
 * startup throw must reach the runtime's window error reporting instead of a
 * local catch.
 */
export function renderCapsuleDocument(input) {
  return `<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${input.head ?? ""}<style>${input.styles}</style></head>
<body>${input.body}<script ${runtimeScriptAttribute}>${escapeInlineScript(input.runtime)}</script><script type="module">${escapeInlineScript(input.script)}</script></body></html>`;
}

/**
 * Builds the artifact from a Capsule.
 *
 * input.paths   every file the Capsule holds, relative and with `/`
 * input.read    path -> string for text, Uint8Array for binary
 * input.compile () -> { styles, script }: styles.css and the bundled app for fragment pages
 * input.bundle  path -> { script, styles }: one module script a document page names
 * input.runtime the SDK source
 * input.head    optional markup for the head of every page
 * input.routing optional "pages" or "spa"
 */
export async function assembleStaticArtifact(input) {
  checkRouting(input.routing);
  const held = new Set(input.paths);
  /** served path -> Capsule path */
  const sources = new Map();
  const claim = (served, source) => {
    const reserved = reservedRoots.find((root) => served === root || served.startsWith(`${root}/`));
    if (reserved) throw new Error(`${source} would publish at /${served}, under /${reserved}, which the Kernel keeps.`);
    if (served === runtimeFile) throw new Error(`${source} would publish at /${runtimeFile}, where the build puts the Littlebox SDK. Rename it.`);
    const other = sources.get(served);
    if (other && other !== source) throw new Error(`${other} and ${source} would both publish at /${served}. Keep one.`);
    sources.set(served, source);
  };
  for (const source of [...held].sort()) {
    const served = servedPath(source);
    if (served) claim(served, source);
  }
  if (held.has(runtimeFile)) throw new Error(`The Capsule holds ${runtimeFile}, the name the build gives the Littlebox SDK. Rename it.`);

  const files = {};
  const encodings = {};
  const put = (served, contents) => {
    if (typeof contents === "string") files[served] = contents;
    else { files[served] = toBase64(contents); encodings[served] = "base64"; }
  };
  const text = async (source) => {
    const contents = await input.read(source);
    return typeof contents === "string" ? contents : new TextDecoder().decode(contents);
  };

  const pages = [...sources].filter(([served]) => served.endsWith(".html") && !sources.get(served).startsWith("public/"));
  if (!sources.has("index.html")) pages.unshift(["index.html", null]);
  let compiled;
  let usesRuntimeFile = false;
  const bundled = new Set();
  const pending = [];
  const reference = (raw, from, kind) => {
    const target = resolveReference(raw, from);
    if (target === null) return;
    if (target === runtimeFile) { usesRuntimeFile = true; return; }
    if (sources.has(target)) {
      if (kind === "module" && !bundled.has(target) && !sources.get(target).startsWith("public/")) pending.push({ target, kind, from, raw });
      return;
    }
    if (!held.has(target)) throw new Error(`${from} references ${raw}, but the Capsule holds no ${target}.`);
    claim(target, target);
    pending.push({ target, kind, from, raw });
  };

  for (const [served, source] of pages) {
    const html = source === null ? defaultBody : await text(source);
    if (!isDocument(html)) {
      compiled ??= await input.compile();
      scanHtml(html, served, reference);
      scanCss(compiled.styles, served, reference);
      files[served] = renderCapsuleDocument({ body: html, styles: compiled.styles, script: compiled.script, runtime: input.runtime, head: input.head });
      continue;
    }
    let page = html;
    let loadsRuntime = false;
    scanHtml(html, served, (raw, from, kind) => {
      if (kind !== "link" && resolveReference(raw, from) === runtimeFile) loadsRuntime = true;
      reference(raw, from, kind);
    });
    const additions = `${input.head ?? ""}${loadsRuntime ? "" : `<script src="/${runtimeFile}"></script>`}`;
    page = insertIntoHead(page, additions);
    usesRuntimeFile = true;
    files[served] = page;
  }

  while (pending.length) {
    const { target, kind, from, raw } = pending.shift();
    const source = sources.get(target);
    if (kind === "module" && !source.startsWith("public/")) {
      if (bundled.has(target)) continue;
      bundled.add(target);
      if (!/\.m?js$/.test(target)) throw new Error(`${from} loads ${raw} as a module. A page loads JavaScript, so name a .js or .mjs file and let the build compile it.`);
      const output = await input.bundle(source);
      if (output.styles) throw new Error(`${source} imports CSS. Link the stylesheet from the page instead.`);
      files[target] = output.script;
      continue;
    }
    const contents = await input.read(source);
    put(target, contents);
    if (target.endsWith(".css")) scanCss(typeof contents === "string" ? contents : new TextDecoder().decode(contents), target, reference);
  }

  for (const [served, source] of sources) {
    if (source.startsWith("public/") && files[served] === undefined) put(served, await input.read(source));
  }
  if (usesRuntimeFile) files[runtimeFile] = input.runtime;
  const artifact = { version: 1, entrypoint: "index.html", files };
  if (Object.keys(encodings).length) artifact.encodings = encodings;
  return artifact;
}

/**
 * The served path a reference names, or null when it is not a Capsule file:
 * another origin, a fragment, a data URL or a Kernel path under /_littlebox.
 */
export function resolveReference(raw, from) {
  const value = raw.trim();
  if (!value || value.startsWith("#") || value.startsWith("//") || /^[a-z][a-z0-9+.-]*:/i.test(value)) return null;
  const bare = value.split(/[?#]/)[0];
  if (!bare) return null;
  let decoded;
  try { decoded = decodeURIComponent(bare); } catch { throw new Error(`${from} references ${raw}, which is not a valid URL path.`); }
  const base = decoded.startsWith("/") ? [] : from.split("/").slice(0, -1);
  const parts = [...base];
  for (const part of decoded.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (!parts.length) throw new Error(`${from} references ${raw}, which leaves the Capsule.`);
      parts.pop();
    } else parts.push(part);
  }
  const target = parts.join("/");
  if (!target || target === "_littlebox" || target.startsWith("_littlebox/")) return null;
  if (decoded.endsWith("/")) return `${target}/index.html`;
  return target;
}

/** Calls `reference(raw, from, kind)` for every file a page fetches when it loads. */
export function scanHtml(html, from, reference) {
  const visible = html.replace(/<!--[\s\S]*?-->/g, "");
  const tags = /<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=\/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+))?)*)\s*\/?>/g;
  let match;
  while ((match = tags.exec(visible))) {
    const tag = match[1].toLowerCase();
    const attributes = parseAttributes(match[2] ?? "");
    const end = tags.lastIndex;
    if (tag === "script" || tag === "style") {
      const close = visible.toLowerCase().indexOf(`</${tag}`, end);
      const body = visible.slice(end, close === -1 ? visible.length : close);
      if (tag === "style") scanCss(body, from, reference);
      tags.lastIndex = close === -1 ? visible.length : close;
    }
    if (attributes.style) scanCss(attributes.style, from, reference);
    if (tag === "script" && attributes.src !== undefined) {
      reference(attributes.src, from, (attributes.type ?? "").toLowerCase() === "module" ? "module" : "asset");
    } else if (tag === "link" && attributes.href !== undefined) {
      const rels = (attributes.rel ?? "").toLowerCase().split(/\s+/);
      if (rels.some((rel) => fetchingRels.includes(rel))) reference(attributes.href, from, rels.includes("modulepreload") ? "module" : "asset");
    } else if (["img", "source", "video", "audio", "track", "embed"].includes(tag)) {
      if (attributes.src !== undefined) reference(attributes.src, from, "asset");
      if (attributes.poster !== undefined) reference(attributes.poster, from, "asset");
      if (attributes.srcset !== undefined) {
        for (const candidate of attributes.srcset.split(",")) {
          const url = candidate.trim().split(/\s+/)[0];
          if (url) reference(url, from, "asset");
        }
      }
    }
  }
}

/** Calls `reference` for every url() and @import in a stylesheet. */
export function scanCss(css, from, reference) {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of clean.matchAll(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]*))\s*\)/gi)) {
    reference(match[1] ?? match[2] ?? match[3] ?? "", from, "asset");
  }
  for (const match of clean.matchAll(/@import\s+(?:"([^"]*)"|'([^']*)')/gi)) {
    reference(match[1] ?? match[2] ?? "", from, "asset");
  }
}

function parseAttributes(source) {
  const attributes = {};
  for (const match of source.matchAll(/([^\s=\/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g)) {
    const name = match[1].toLowerCase();
    if (!(name in attributes)) attributes[name] = decodeEntities(match[2] ?? match[3] ?? match[4] ?? "");
  }
  return attributes;
}

function decodeEntities(value) {
  return value.replace(/&(amp|quot|#39|apos|lt|gt);/g, (_, name) => ({ amp: "&", quot: '"', "#39": "'", apos: "'", lt: "<", gt: ">" })[name]);
}

/** Puts markup first in the head, so the SDK runs before any script the page names. */
function insertIntoHead(html, markup) {
  if (!markup) return html;
  const head = /<head(?:\s[^>]*)?>/i.exec(html);
  if (head) return html.slice(0, head.index + head[0].length) + markup + html.slice(head.index + head[0].length);
  const root = /<html(?:\s[^>]*)?>/i.exec(html);
  if (root) return html.slice(0, root.index + root[0].length) + `<head>${markup}</head>` + html.slice(root.index + root[0].length);
  const doctype = /<!doctype[^>]*>/i.exec(html);
  const at = doctype ? doctype.index + doctype[0].length : 0;
  return html.slice(0, at) + `<head>${markup}</head>` + html.slice(at);
}

function escapeInlineScript(value) {
  return value.replaceAll("</script", "<\\/script");
}

function toBase64(bytes) {
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  }
  return btoa(binary);
}
