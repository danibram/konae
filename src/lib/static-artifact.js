
export const runtimeScriptAttribute = "data-konae-runtime";

export const runtimeFile = "runtime.js";
export const sourceOnlyFiles = ["schema-transforms.json"];

export const pathPrefixPattern = /^\/p\/([^\/]+)(\/|$)/;

export function basePath(pathname) {
  const match = pathPrefixPattern.exec(pathname);
  return match ? `/p/${match[1]}` : "";
}

export function runtimeLoaderScript() {
  return `<script>(function(){var m=/${pathPrefixPattern.source}/.exec(location.pathname);document.write('<script src="'+(m?'/p/'+m[1]:'')+'/${runtimeFile}"><\\/script>')})();</script>`;
}

export const routingModes = ["pages", "spa"];
export const routingMessage = 'Routing must be "pages" or "spa".';

const reservedRoots = ["_konae", "oauth", ".well-known", "mcp"];
const fetchingRels = ["stylesheet", "icon", "apple-touch-icon", "mask-icon", "manifest", "preload", "modulepreload", "prefetch"];
const defaultBody = "<main id=app></main>";

export function checkRouting(value) {
  if (value === undefined || value === null) return "pages";
  if (!routingModes.includes(value)) throw new Error(routingMessage);
  return value;
}

export function servedPath(source) {
  if (source.startsWith("public/")) return source.slice("public/".length) || null;
  if (!source.endsWith(".html")) return null;
  if (!source.includes("/")) return source;
  if (source.startsWith("pages/")) return source.slice("pages/".length);
  return null;
}

export function isDocument(html) {
  return /^\s*(?:<!--[\s\S]*?-->\s*)*<(?:!doctype|html)[\s>]/i.test(html);
}

export const appEntries = ["src/main.tsx", "src/main.ts", "app.js"];

export function appEntry(paths) {
  const held = new Set(paths);
  return appEntries.find((entry) => held.has(entry)) ?? null;
}

export function entryNeverRunsMessage(paths, pages) {
  const list = [...paths];
  const entry = appEntry(list);
  if (entry === null) return null;
  const sources = list.filter((source) => !source.startsWith("public/") && servedPath(source)?.endsWith(".html"));
  const index = sources.find((source) => servedPath(source) === "index.html");
  if (index === undefined) return null;
  for (const source of sources) {
    const html = pages[source];
    if (typeof html !== "string" || !isDocument(html) || loadsScript(html, servedPath(source))) return null;
  }
  const load = entry.endsWith(".js") ? `add <script type="module" src="${entry}"></script> to it` : `add a <script type="module" src> that names a .js file importing ${entry}`;
  return `${index} is a whole document that loads no script, so ${entry} never runs and the page stays empty. Drop its doctype and its <html>, <head> and <body> tags so the build puts ${entry} into it, or ${load}.`;
}

function loadsScript(html, from) {
  const visible = html.replace(/<!--[\s\S]*?-->/g, "");
  for (const match of visible.matchAll(/<script\b((?:\s+[^\s=\/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+))?)*)\s*\/?>([\s\S]*?)(?:<\/script\s*>|$)/gi)) {
    const src = parseAttributes(match[1] ?? "").src;
    if (src === undefined) {
      if (match[2].trim()) return true;
      continue;
    }
    let target;
    try { target = resolveReference(src, from); } catch { return true; }
    if (target !== runtimeFile) return true;
  }
  return false;
}

export function renderCapsuleDocument(input) {
  const { head, body } = splitFragmentHead(input.body);
  return `<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${input.head ?? ""}${head}<style>${input.styles}</style></head>
<body>${body}<script ${runtimeScriptAttribute}>${escapeInlineScript(input.runtime)}</script><script type="module">${escapeInlineScript(input.script)}</script></body></html>`;
}

export function splitFragmentHead(fragment) {
  const lead = /^\s*(?:<!--[\s\S]*?-->|<title(?:\s[^>]*)?>[^<]*<\/title\s*>|<link\s[^>]*>)/i;
  const moved = [];
  let rest = fragment;
  for (let match = lead.exec(rest); match; match = lead.exec(rest)) {
    const element = match[0].trim();
    if (/^<link/i.test(element) && !/\bicon\b/i.test(parseAttributes(element.slice(5)).rel ?? "")) break;
    if (!element.startsWith("<!--")) moved.push(element);
    rest = rest.slice(match[0].length);
  }
  return moved.length ? { head: moved.join(""), body: rest.replace(/^\s+/, "") } : { head: "", body: fragment };
}

export async function assembleStaticArtifact(input) {
  checkRouting(input.routing);
  const sourceOnly = new Set(sourceOnlyFiles);
  const held = new Set(input.paths.filter((path) => !sourceOnly.has(path)));
  const sources = new Map();
  const claim = (served, source) => {
    const reserved = reservedRoots.find((root) => served === root || served.startsWith(`${root}/`));
    if (reserved) throw new Error(`${source} would publish at /${served}, under /${reserved}, which the Kernel keeps.`);
    if (served === runtimeFile) throw new Error(`${source} would publish at /${runtimeFile}, where the build puts the Konae SDK. Rename it.`);
    const other = sources.get(served);
    if (other && other !== source) throw new Error(`${other} and ${source} would both publish at /${served}. Keep one.`);
    sources.set(served, source);
  };
  for (const source of [...held].sort()) {
    const served = servedPath(source);
    if (served) claim(served, source);
  }
  if (held.has(runtimeFile)) throw new Error(`The Capsule holds ${runtimeFile}, the name the build gives the Konae SDK. Rename it.`);

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
  const seen = {};
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

  for (const [, source] of pages) if (source !== null) seen[source] = await text(source);
  const neverRuns = entryNeverRunsMessage(held, seen);
  if (neverRuns) throw new Error(neverRuns);

  for (const [served, source] of pages) {
    const sourceHtml = source === null ? defaultBody : seen[source];
    const bom = sourceHtml.startsWith("\uFEFF") ? "\uFEFF" : "";
    const html = bom ? sourceHtml.slice(1) : sourceHtml;
    if (!isDocument(html)) {
      compiled ??= withoutModulePaths(await input.compile(), held);
      scanHtml(html, served, reference);
      scanCss(compiled.styles, served, reference);
      files[served] = bom + renderCapsuleDocument({ body: html, styles: compiled.styles, script: compiled.script, runtime: input.runtime, head: input.head });
      continue;
    }
    scanHtml(html, served, reference);
    const page = insertIntoHead(withoutRuntimeTags(html, served), `${input.head ?? ""}${runtimeLoaderScript()}`);
    usesRuntimeFile = true;
    files[served] = bom + page;
  }

  while (pending.length) {
    const { target, kind, from, raw } = pending.shift();
    const source = sources.get(target);
    if (kind === "module" && !source.startsWith("public/")) {
      if (bundled.has(target)) continue;
      bundled.add(target);
      if (!/\.m?js$/.test(target)) throw new Error(`${from} loads ${raw} as a module. A page loads JavaScript, so name a .js or .mjs file and let the build compile it.`);
      const output = withoutModulePaths(await input.bundle(source), held);
      if (output.styles) throw new Error(`${source} imports CSS. Link the stylesheet from the page instead.`);
      files[target] = output.script;
      continue;
    }
    const contents = await input.read(source);
    put(target, contents);
    if (target.endsWith(".css")) {
      const css = typeof contents === "string" ? contents : new TextDecoder().decode(contents);
      seen[source] = css;
      scanCss(css, target, reference);
    }
  }

  for (const [served, source] of sources) {
    if (source.startsWith("public/") && files[served] === undefined) {
      const contents = await input.read(source);
      put(served, contents);
      if (source.endsWith(".html")) seen[source] = typeof contents === "string" ? contents : new TextDecoder().decode(contents);
    }
  }
  if (usesRuntimeFile) files[runtimeFile] = input.runtime;
  for (const { message } of rootRelativeWarnings(seen)) input.warn?.(message);
  const artifact = { version: 1, entrypoint: "index.html", files };
  if (Object.keys(encodings).length) artifact.encodings = encodings;
  return artifact;
}

export function stripModulePaths(code, held) {
  const named = (path) => held.has(path.startsWith("konae:") ? path.slice("konae:".length) : path);
  return code.replace(/^(?:\/\/ (\S+)|\/\* (\S+) \*\/)\n/gm, (line, js, css) => named(js ?? css) ? "" : line);
}

function withoutModulePaths(output, held) {
  return { ...output, script: stripModulePaths(output.script, held), styles: output.styles && stripModulePaths(output.styles, held) };
}

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
  if (!target || target === "_konae" || target.startsWith("_konae/")) return null;
  if (decoded.endsWith("/")) return `${target}/index.html`;
  return target;
}

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

export function scanCss(css, from, reference) {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of clean.matchAll(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]*))\s*\)/gi)) {
    reference(match[1] ?? match[2] ?? match[3] ?? "", from, "asset");
  }
  for (const match of clean.matchAll(/@import\s+(?:"([^"]*)"|'([^']*)')/gi)) {
    reference(match[1] ?? match[2] ?? "", from, "asset");
  }
}

export function isRootRelativeUrl(raw) {
  const value = raw.trim();
  return value.startsWith("/") && !value.startsWith("//") && value !== `/${runtimeFile}`;
}

export function rootRelativeWarnings(files) {
  const assetAt = (source) => (source.startsWith("public/") ? source.slice("public/".length) : source);
  const heldAt = new Map(Object.keys(files).map((source) => [assetAt(source), source]));
  const warnings = [];
  const messages = new Set();
  const sheets = [];
  const queued = new Set();
  const warn = (source, from, raw, tag, attribute) => {
    if (!isRootRelativeUrl(raw)) return;
    const message = rootRelativeMessage(source, from, raw.trim(), verbFor(tag, attribute));
    if (messages.has(message)) return;
    messages.add(message);
    warnings.push({ source, message });
  };
  const queue = (raw, from) => {
    let path;
    try { path = resolveReference(raw, from); } catch { return; }
    const source = path === null ? undefined : heldAt.get(path);
    if (source === undefined || !source.endsWith(".css") || queued.has(source)) return;
    queued.add(source);
    sheets.push(source);
  };

  for (const source of Object.keys(files).sort()) {
    const served = servedPath(source);
    if (served === null || !served.endsWith(".html")) continue;
    walkHtml(files[source], served, (raw, tag, attribute, rels) => {
      warn(source, served, raw, tag, attribute);
      if (tag === "link" && rels.includes("stylesheet")) queue(raw, served);
    });
  }
  for (let index = 0; index < sheets.length; index += 1) {
    const source = sheets[index];
    const served = assetAt(source);
    walkCss(files[source], (raw, attribute) => {
      warn(source, served, raw, "style", attribute);
      if (attribute === "@import") queue(raw, served);
    });
  }
  return warnings;
}

function rootRelativeMessage(source, served, url, verb) {
  const depth = served.split("/").length - 1;
  const up = depth ? "../".repeat(depth) : "./";
  const rest = url.slice(1);
  let relative;
  if (rest === "") relative = `${up}index.html`;
  else if (rest.startsWith("?") || rest.startsWith("#") || depth) relative = `${up}${rest}`;
  else relative = rest;
  return `${source} ${verb} ${url}, which leaves the app's path address. Write ${relative} instead.`;
}

function verbFor(tag, attribute) {
  if (attribute === "action" || attribute === "formaction") return "submits to";
  if (attribute === "href" && (tag === "a" || tag === "area")) return "links to";
  return "loads";
}

function walkHtml(html, from, visit) {
  const visible = html.replace(/<!--[\s\S]*?-->/g, "");
  const tags = /<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=\/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+))?)*)\s*\/?>/g;
  let match;
  while ((match = tags.exec(visible))) {
    const tag = match[1].toLowerCase();
    const attributes = parseAttributes(match[2] ?? "");
    const end = tags.lastIndex;
    if (tag === "script" || tag === "style") {
      const close = visible.toLowerCase().indexOf(`</${tag}`, end);
      if (tag === "style") walkCss(visible.slice(end, close === -1 ? visible.length : close), (raw, attribute) => visit(raw, tag, attribute, []));
      tags.lastIndex = close === -1 ? visible.length : close;
    }
    if (attributes.style !== undefined) walkCss(attributes.style, (raw, attribute) => visit(raw, tag, attribute, []));
    const rels = (attributes.rel ?? "").toLowerCase().split(/\s+/);
    for (const name of ["href", "src", "action", "formaction", "poster", "data"]) {
      if (attributes[name] !== undefined) visit(attributes[name], tag, name, rels);
    }
    if (attributes.srcset !== undefined) {
      for (const candidate of attributes.srcset.split(",")) {
        const url = candidate.trim().split(/\s+/)[0];
        if (url) visit(url, tag, "srcset", rels);
      }
    }
  }
}

function walkCss(css, visit) {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of clean.matchAll(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]*))\s*\)/gi)) visit(match[1] ?? match[2] ?? match[3] ?? "", "url");
  for (const match of clean.matchAll(/@import\s+(?:"([^"]*)"|'([^']*)')/gi)) visit(match[1] ?? match[2] ?? "", "@import");
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

function withoutRuntimeTags(html, from) {
  return html.replace(/<script\b([^>]*)>\s*<\/script\s*>/gi, (tag, attributes) => {
    const src = parseAttributes(attributes).src;
    return src !== undefined && resolveReference(src, from) === runtimeFile ? "" : tag;
  });
}

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
