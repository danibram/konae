import type { HostedDeployment } from "./store";

const hostedRuntime = String.raw`
(() => {
  async function call(path, init) {
    const response = await fetch(path, {
      ...init,
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json', ...(init && init.headers) }
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error || 'Littlebox Kernel request failed');
    return body.result;
  }
  window.miniapp = {
    actions: { call: (name, input = {}) => call('/_littlebox/actions/' + encodeURIComponent(name), { method: 'POST', body: JSON.stringify(input) }) },
    auth: { session: () => call('/_littlebox/session'), signIn: () => { window.location.assign('/_littlebox/login'); }, signOut: () => call('/_littlebox/auth/logout', { method: 'POST', body: '{}' }) },
    files: {
      list: () => call('/_littlebox/files'),
      upload: async (file) => {
        const form = new FormData();
        form.append('file', file);
        const response = await fetch('/_littlebox/files', { method: 'POST', credentials: 'same-origin', body: form });
        const body = await response.json().catch(() => ({}));
        if (!response.ok) throw new Error(body.error || 'File upload failed');
        return body.result;
      }
    }
  };
})();`;

export function renderDeployment(deployment: HostedDeployment): string {
  const code = escapeInlineScript(deployment.files["app.js"]);
  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="littlebox-deployment" content="${escapeHtml(deployment.id)}">
    <style>${deployment.files["styles.css"]}</style>
  </head>
  <body>
    ${deployment.files["index.html"]}
    <script>${hostedRuntime}</script>
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

export function renderPrivateLogin(deployment: HostedDeployment, error = ""): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Private · ${escapeHtml(deployment.projectName)}</title><style>
  :root{font-family:ui-monospace,SFMono-Regular,monospace;color:#20211d;background:#efede4}*{box-sizing:border-box}body{min-height:100vh;margin:0;display:grid;place-items:center;background-image:radial-gradient(#c5c1b6 1px,transparent 1px);background-size:14px 14px}.card{width:min(430px,calc(100vw - 36px));padding:32px;border:1px solid #20211d;background:#f7f4eb;box-shadow:9px 9px 0 #c8f34a}.mark{width:48px;height:48px;display:grid;place-items:center;border:1px solid #20211d;background:#20211d;color:white;font-size:22px}small{letter-spacing:.12em;color:#74766d}h1{font-family:system-ui,sans-serif;font-size:38px;line-height:.95;letter-spacing:-.05em;margin:20px 0 10px}p{color:#676960;font:12px/1.55 system-ui,sans-serif}.error{padding:9px;border-left:3px solid #f06445;background:#f1ded6;color:#7a2d1d}label{display:block;margin-top:22px;font-size:10px;font-weight:700}input{width:100%;margin-top:7px;padding:12px;border:1px solid #20211d;background:white;font:15px ui-monospace,monospace;outline:none}button{width:100%;margin-top:12px;padding:12px;border:1px solid #20211d;background:#c8f34a;font-weight:800;box-shadow:3px 3px 0 #20211d;cursor:pointer}
  </style></head><body><main class="card"><div class="mark">⌁</div><small>LITTLEBOX / PRIVATE DEPLOYMENT</small><h1>${escapeHtml(deployment.projectName)}</h1><p>This deployment is protected by the Littlebox Kernel. Enter an access code issued by the project owner.</p>${error ? `<p class="error">${escapeHtml(error)}</p>` : ""}<form method="post" action="/_littlebox/login"><label>ACCESS CODE<input name="code" type="password" autocomplete="one-time-code" required autofocus></label><button>Unlock deployment</button></form></main></body></html>`;
}

function escapeInlineScript(value: string) {
  return value.replaceAll("</script", "<\\/script");
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[character] ?? character);
}
