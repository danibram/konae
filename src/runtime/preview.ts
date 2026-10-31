import type { MiniAppProject } from "../lib/types";
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
    // Last connect wins: the host may hand over a fresh port after a reload or a
    // missed handshake, and refusing it would strand the app on a dead one.
    if (runtimePort) runtimePort.close();
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
        const visibility = options && options.visibility !== undefined ? options.visibility : 'private';
        if (visibility !== 'private' && visibility !== 'public') throw new Error('File visibility must be private or public.');
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error);
        reader.onload = () => request('files.upload', {
          name: file.name,
          type: file.type,
          size: file.size,
          dataUrl: reader.result,
          visibility
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
  return renderCapsuleDocument({
    body: project.files["index.html"] ?? "",
    styles: project.files["styles.css"] ?? "",
    script: project.files["app.js"] ?? "",
    runtime: runtimeClient,
  });
}
