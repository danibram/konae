export type BuildArtifact = {
  version: 1;
  entrypoint: "index.html";
  files: Record<string, string>;
};

const hostedRuntime = String.raw`(() => {
  async function call(path, init) {
    const response = await fetch(path, { ...init, credentials: 'same-origin', headers: { 'content-type': 'application/json', ...(init && init.headers) } });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error || 'Littlebox Kernel request failed');
    return body.result;
  }
  window.miniapp = {
    actions: { call: (name, input = {}) => call('/_littlebox/actions/' + encodeURIComponent(name), { method: 'POST', body: JSON.stringify(input) }) },
    auth: {
      session: () => call('/_littlebox/session'),
      signIn: () => { window.location.assign('/_littlebox/login'); },
      signOut: () => call('/_littlebox/auth/logout', { method: 'POST', body: '{}' })
    },
    files: {
      list: () => call('/_littlebox/files'),
      upload: (file) => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(reader.error);
        reader.onload = () => call('/_littlebox/files', {
          method: 'POST', body: JSON.stringify({ name: file.name, type: file.type, size: file.size, dataUrl: reader.result })
        }).then(resolve, reject);
        reader.readAsDataURL(file);
      })
    }
  };
})();`;

export function createHostedArtifact(input: {
  body: string;
  styles: string;
  script: string;
  deploymentId?: string;
}): BuildArtifact {
  const deploymentMeta = input.deploymentId
    ? `<meta name="littlebox-deployment" content="${escapeHtml(input.deploymentId)}">`
    : "";
  return {
    version: 1,
    entrypoint: "index.html",
    files: {
      "index.html": `<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${deploymentMeta}<style>${input.styles}</style></head>
<body>${input.body}<script>${hostedRuntime}</script><script type="module">${escapeInlineScript(input.script)}</script></body></html>`,
    },
  };
}

function escapeInlineScript(value: string) {
  return value.replaceAll("</script", "<\\/script");
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[character] ?? character);
}
