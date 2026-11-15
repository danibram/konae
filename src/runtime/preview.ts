import type { MiniAppProject } from "../lib/types";
import { sourceBytes } from "../lib/source-files";
import { resolveReference, servedPath } from "../lib/static-artifact.js";
import { renderCapsuleDocument } from "./artifact";

export const runtimeClient = String.raw`
(() => {
  const pending = new Map();
  let sequence = 0;
  let runtimePort;
  let connectRuntime;
  const runtimeReady = new Promise((resolve) => { connectRuntime = resolve; });

  let capability;
  const methods = { 'actions.invoke': 'actions_invoke', 'auth.session': 'auth_session', 'auth.signIn': 'preview_sign_in', 'auth.signInWithGoogle': 'preview_sign_in_with_google', 'auth.signOut': 'auth_logout', 'files.list': 'files_list', 'files.upload': 'preview_files_upload', 'files.url': 'preview_files_url', 'files.update': 'files_update', 'files.delete': 'files_delete', 'runtime.ready': 'preview_ready', 'preview.error': 'preview_error' };
  function rpc(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = 'req_' + (++sequence);
      pending.set(id, { resolve, reject });
      runtimePort.postMessage({ jsonrpc: '2.0', id, method, params: { ...params, _meta: { 'konae/capability': capability } } });
    });
  }
  function request(method, payload) {
    return runtimeReady.then(() => rpc('tools/call', { name: methods[method], arguments: payload || {} })).then(result => {
      if (result.isError) throw new Error(result.content?.[0]?.text || 'Preview tool failed');
      return result.structuredContent.result;
    });
  }

  addEventListener('message', (event) => {
    if (event.source !== parent || !event.ports[0]) return;
    const message = event.data;
    if (!message || message.channel !== 'konae-runtime' || message.type !== 'connect') return;
    // Last connect wins for new requests: the host may hand over a fresh port
    // after a reload or a missed handshake, and refusing it would strand the
    // app on a dead one. The previous port stays open, so a request already
    // sent on it still gets its answer.
    runtimePort = event.ports[0];
    capability = message.capability;
    runtimePort.onmessage = ({ data: response }) => {
      if (!response || response.jsonrpc !== '2.0') return;
      const handler = pending.get(response.id);
      if (!handler) return;
      pending.delete(response.id);
      if (response.error) handler.reject(new Error(response.error.message || 'Preview MCP failed'));
      else handler.resolve(response.result);
    };
    runtimePort.start();
    rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'konae-preview-app', version: '0.1.0' } }).then(() => {
      runtimePort.postMessage({ jsonrpc: '2.0', method: 'notifications/initialized', params: { _meta: { 'konae/capability': capability } } });
      connectRuntime();
    }).catch(() => {});
  });

  /**
   * A capsule can die before any of its listeners attach — a parse error in
   * app.js never reaches a try/catch. Reporting from window makes that visible
   * to the owner and readable by the agent instead of a preview that silently
   * does nothing.
   */
  let reporting = false;
  function report(kind, message, stack) {
    if (reporting) return;
    reporting = true;
    const box = document.createElement('pre');
    box.dataset.konaeError = kind;
    box.style.cssText = 'margin:20px;padding:16px;background:#3b1717;color:#ffdede;white-space:pre-wrap';
    box.textContent = message + (stack ? '\n\n' + stack : '');
    document.body.appendChild(box);
    request('preview.error', { kind, message, stack }).catch(() => {});
    reporting = false;
  }

  addEventListener('error', (event) => {
    const error = event.error;
    report('error', event.message || String(error), error && error.stack);
  });
  addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    report('unhandledrejection', reason && reason.message ? reason.message : String(reason), reason && reason.stack);
  });

  /**
   * Announce readiness instead of trusting the host to catch our load event: a
   * srcDoc document can finish loading before the host attaches its listener,
   * and a handshake that depends on winning that race silently strands the app.
   */
  try {
    parent.postMessage({ channel: 'konae-runtime', type: 'hello' }, '*');
  } catch (error) {
    /* no parent to greet */
  }

  window.miniapp = {
    actions: {
      call: (name, input = {}) => request('actions.invoke', { name, input })
    },
    auth: {
      session: () => request('auth.session'),
      signIn: () => request('auth.signIn'),
      signInWithGoogle: () => request('auth.signInWithGoogle'),
      signOut: () => request('auth.signOut')
    },
    files: {
      list: () => request('files.list'),
      upload: (file, options) => new Promise((resolve, reject) => {
        const visibility = options && options.visibility;
        if (visibility !== undefined && visibility !== 'private' && visibility !== 'public' && visibility !== 'app-users') throw new Error('File visibility must be private or public.');
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error);
        reader.onload = () => request('files.upload', {
          name: file.name,
          type: file.type,
          size: file.size,
          dataUrl: reader.result,
          ...(visibility === undefined ? {} : { visibility })
        }).then(resolve, reject);
        reader.readAsDataURL(file);
      }),
      url: (fileId) => request('files.url', { fileId }),
      update: (fileId, options) => request('files.update', { fileId, visibility: options && options.visibility }),
      delete: (fileId) => request('files.delete', { fileId })
    }
  };
})();`;

/**
 * The preview must run a Capsule exactly as a Deployment does, same module
 * semantics and same error handling, or an app can work in one and be dead in
 * the other. Both go through renderCapsuleDocument; only the SDK transport
 * differs.
 */
export function createPreviewDocument(project: MiniAppProject): string {
  const assets = new Map<string, string>();
  const stylesheets = new Map<string, string>();
  for (const source of Object.keys(project.files)) {
    if (!source.startsWith("public/") && !source.endsWith(".css")) continue;
    const served = servedPath(source);
    const bytes = sourceBytes(project, source);
    if (served && source.startsWith("public/")) assets.set(served, `data:${mediaType(served)};base64,${bytesToBase64(bytes)}`);
    if (source.endsWith(".css")) {
      try { stylesheets.set(served ?? source, new TextDecoder("utf-8", { fatal: true }).decode(bytes)); } catch { /* A binary file cannot be a stylesheet. */ }
    }
  }
  const document = renderCapsuleDocument({
    body: project.files["index.html"] ?? "",
    styles: project.files["styles.css"] ?? "",
    script: project.files["app.js"] ?? "",
    runtime: runtimeClient,
  });
  return rewritePreviewAssets(document, assets, stylesheets);
}

/** Rewrites only static fetch attributes and CSS URLs the shared builder resolves. */
function rewritePreviewAssets(document: string, assets: Map<string, string>, stylesheets: Map<string, string>): string {
  const rewriteCss = (css: string, from: string, visited = new Set<string>()): string => {
    let output = css.replace(/@import\s+(?:url\(\s*)?(["'])([^"']+)\1\s*\)?([^;]*);/gi, (whole, quote, raw, suffix) => {
      const target = resolveReferenceOrNull(raw, from);
      const sheet = target && stylesheets.get(target);
      if (!sheet || visited.has(target)) return whole;
      const imported = rewriteCss(sheet, target, new Set(visited).add(target));
      const media = suffix.trim();
      return media ? `@media ${media}{${imported}}` : imported;
    });
    output = output.replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*?))\s*\)/gi, (whole, double, single, bare) => {
      const raw = (double ?? single ?? bare ?? "").trim();
      const target = previewAsset(raw, from, assets);
      if (!target) return whole;
      const quote = double !== undefined ? '"' : single !== undefined ? "'" : "";
      return `url(${quote}${target}${quote})`;
    });
    return output;
  };

  const rewriteMarkup = (markup: string) => markup.replace(/<(?!script\b)([a-z][\w-]*)(\s[^<>]*?)?>/gi, (_tag, name: string, attributes = "") => {
    const kind = name.toLowerCase();
    const rel = readAttribute(attributes, "rel") ?? "";
    const href = readAttribute(attributes, "href");
    if (kind === "link" && rel.toLowerCase().split(/\s+/).includes("stylesheet") && href) {
      const target = resolveReferenceOrNull(href, "index.html");
      const sheet = target && stylesheets.get(target);
      if (sheet) return `<style>${rewriteCss(sheet, target)}</style>`;
    }
    const isIcon = kind === "link" && /(?:^|\s)(?:icon|apple-touch-icon|mask-icon)(?:\s|$)/i.test(rel);
    const permitted = kind === "img" || kind === "source" || isIcon;
    let next = attributes;
    if (permitted) {
      next = next.replace(/(^|\s)(src|href)(\s*=\s*)("[^"]*"|'[^']*'|[^\s>]+)/gi, (attribute: string, prefix: string, key: string, separator: string, quoted: string) => {
        if (key.toLowerCase() === "href" && !isIcon) return attribute;
        const quote = quoted[0] === '"' || quoted[0] === "'" ? quoted[0] : "";
        const raw = quote ? quoted.slice(1, -1) : quoted;
        const target = previewAsset(raw, "index.html", assets);
        return target ? `${prefix}${key}${separator}${quote}${target}${quote}` : attribute;
      });
      next = next.replace(/(^|\s)(srcset)(\s*=\s*)("[^"]*"|'[^']*'|[^\s>]+)/gi, (attribute: string, prefix: string, key: string, separator: string, quoted: string) => {
        const quote = quoted[0] === '"' || quoted[0] === "'" ? quoted[0] : "";
        const value = quote ? quoted.slice(1, -1) : quoted;
        if (/^\s*data:/i.test(value)) return attribute;
        const updated = value.split(",").map((candidate: string) => {
          const match = /^(\s*)(\S+)([\s\S]*)$/.exec(candidate);
          if (!match) return candidate;
          const target = previewAsset(match[2], "index.html", assets);
          return target ? `${match[1]}${target}${match[3]}` : candidate;
        }).join(",");
        return `${prefix}${key}${separator}${quote}${updated}${quote}`;
      });
    }
    next = next.replace(/(^|\s)(style)(\s*=\s*)("[^"]*"|'[^']*')/gi, (_attribute: string, prefix: string, key: string, separator: string, quoted: string) => {
      const quote = quoted[0];
      return `${prefix}${key}${separator}${quote}${rewriteCss(quoted.slice(1, -1), "index.html")}${quote}`;
    });
    return `<${name}${next}>`;
  }).replace(/<style\b([^>]*)>([\s\S]*?)<\/style\s*>/gi, (_whole, attributes, css) => `<style${attributes}>${rewriteCss(css, "index.html")}</style>`);
  return document.split(/(<script\b[\s\S]*?<\/script\s*>)/gi).map((part, index) => index % 2 ? part : rewriteMarkup(part)).join("");
}

function readAttribute(attributes: string, name: string): string | undefined {
  const match = new RegExp(`(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, "i").exec(attributes);
  return match?.[1] ?? match?.[2] ?? match?.[3];
}

function resolveReferenceOrNull(raw: string, from: string): string | null {
  try { return resolveReference(raw, from); } catch { return null; }
}

function previewAsset(raw: string, from: string, assets: Map<string, string>): string | null {
  let target: string | null;
  try { target = resolveReference(raw, from); } catch { return null; }
  return target === null ? null : assets.get(target) ?? null;
}

function mediaType(path: string): string {
  const extension = path.split(".").pop()?.toLowerCase();
  return ({ png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", ico: "image/x-icon", avif: "image/avif", woff: "font/woff", woff2: "font/woff2", ttf: "font/ttf", otf: "font/otf" } as Record<string, string>)[extension ?? ""] ?? "application/octet-stream";
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  return btoa(binary);
}
