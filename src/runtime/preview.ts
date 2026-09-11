import type { MiniAppProject } from "../lib/types";

const runtimeClient = String.raw`
(() => {
  const pending = new Map();
  let sequence = 0;
  let runtimePort;
  let connectRuntime;
  const runtimeReady = new Promise((resolve) => { connectRuntime = resolve; });

  function request(method, payload) {
    return runtimeReady.then(() => new Promise((resolve, reject) => {
      const requestId = 'req_' + (++sequence);
      pending.set(requestId, { resolve, reject });
      runtimePort.postMessage({
        channel: 'littlebox-runtime',
        type: 'request',
        requestId,
        method,
        payload
      });
    }));
  }

  addEventListener('message', (event) => {
    if (event.source !== parent || !event.ports[0]) return;
    const message = event.data;
    if (!message || message.channel !== 'littlebox-runtime' || message.type !== 'connect') return;
    // Last connect wins: the host may hand over a fresh port after a reload or a
    // missed handshake, and refusing it would strand the app on a dead one.
    if (runtimePort) runtimePort.close();
    runtimePort = event.ports[0];
    runtimePort.onmessage = ({ data: response }) => {
      if (!response || response.channel !== 'littlebox-runtime' || response.type !== 'response') return;
      const handler = pending.get(response.requestId);
      if (!handler) return;
      pending.delete(response.requestId);
      if (response.ok) handler.resolve(response.result);
      else handler.reject(new Error(response.error || 'Runtime call failed'));
    };
    runtimePort.start();
    connectRuntime();
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
    box.dataset.littleboxError = kind;
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
    parent.postMessage({ channel: 'littlebox-runtime', type: 'hello' }, '*');
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
      signOut: () => request('auth.signOut')
    },
    files: {
      list: () => request('files.list'),
      upload: (file) => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error);
        reader.onload = () => request('files.upload', {
          name: file.name,
          type: file.type,
          size: file.size,
          dataUrl: reader.result
        }).then(resolve, reject);
        reader.readAsDataURL(file);
      })
    }
  };
})();`;

function escapeInlineScript(value: string): string {
  return value.replaceAll("</script", "<\\/script");
}

/**
 * The preview must run a Capsule exactly as a Deployment does — same module
 * semantics, same error handling — or an app can work in one and be dead in the
 * other. Keep this shape in step with the Kernel's renderDeployment.
 */
export function createPreviewDocument(project: MiniAppProject): string {
  const code = escapeInlineScript(project.files["app.js"]);
  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>${project.files["styles.css"]}</style>
  </head>
  <body>
    ${project.files["index.html"]}
    <script>${runtimeClient}</script>
    <script type="module">
      try {
        ${code}
      } catch (error) {
        const pre = document.createElement('pre');
        pre.style.cssText = 'margin:20px;padding:16px;background:#3b1717;color:#ffdede;white-space:pre-wrap';
        pre.textContent = error.stack || error.message;
        document.body.append(pre);
      }
    </script>
  </body>
</html>`;
}
