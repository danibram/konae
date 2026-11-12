(() => {
  // A path address answers at /p/<id-or-name>/ and the Kernel strips that prefix, so every Kernel URL carries it.
  const prefix = /^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);
  const base = prefix ? '/p/' + prefix[1] : '';
  const endpoint = base + '/_konae/mcp';
  const urls = new Set();
  let connected;

  async function rpc(method, params, notification = false) {
    const id = crypto.randomUUID();
    const response = await fetch(endpoint, {
      method: 'POST', credentials: 'same-origin',
      headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', 'MCP-Protocol-Version': '2025-11-25' },
      body: JSON.stringify({ jsonrpc: '2.0', ...(!notification ? { id } : {}), method, ...(params ? { params } : {}) }),
    });
    if (notification && response.status === 202) return;
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(typeof body.error === 'string' ? body.error : 'Runtime MCP request failed.');
      error.status = response.status;
      throw error;
    }
    if (body.jsonrpc !== '2.0' || body.id !== id) throw Error('Invalid MCP response.');
    if (body.error) throw Error(body.error.message);
    return body.result;
  }

  async function call(name, args = {}) {
    if (!connected) connected = (async () => {
      await rpc('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'konae-app', version: '0.1.0' } });
      await rpc('notifications/initialized', undefined, true);
    })().catch(error => { connected = undefined; throw error; });
    await connected;
    const result = await rpc('tools/call', { name, arguments: args });
    if (result.isError) {
      const error = new Error(result.content?.[0]?.text || 'Runtime tool failed.');
      // A visitor without a session gets this for anything not declared anonymous.
      if (error.message === 'authentication-required') error.status = 401;
      throw error;
    }
    return result.structuredContent.result;
  }

  function base64(bytes) {
    let text = '';
    for (const byte of bytes) text += String.fromCharCode(byte);
    return btoa(text);
  }

  async function checksum(bytes) {
    return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), byte => byte.toString(16).padStart(2, '0')).join('');
  }

  async function download(file) {
    const transfer = await call('files_read', { fileId: file.id });
    try {
      if (!Number.isSafeInteger(transfer.size) || transfer.size < 0 || (Number.isSafeInteger(file.size) && transfer.size !== file.size)) throw Error('Invalid file size.');
      const bytes = new Uint8Array(transfer.size);
      let offset = 0;
      while (offset < bytes.length) {
        const block = await call('transfer_read', { transferId: transfer.transferId, offset });
        const data = Uint8Array.from(atob(block.data), c => c.charCodeAt(0));
        if (block.offset !== offset || !data.length || data.length > 65536 || block.nextOffset !== offset + data.length || block.nextOffset > bytes.length) throw Error('Invalid file block.');
        bytes.set(data, offset);
        offset = block.nextOffset;
      }
      if (await checksum(bytes) !== transfer.sha256) throw Error('File checksum mismatch.');
      const inlineTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif'];
      const safeType = inlineTypes.includes(file.type) ? file.type : 'application/octet-stream';
      const url = URL.createObjectURL(new Blob([bytes], { type: safeType }));
      urls.add(url);
      return { ...file, url };
    } finally { await call('transfer_cancel', { transferId: transfer.transferId }).catch(() => {}); }
  }

  function fileId(id) {
    if (typeof id !== 'string' || !id) throw Error('A file ID is required.');
    return id;
  }

  function visibility(value) {
    if (value !== 'private' && value !== 'public' && value !== 'app-users') throw Error('File visibility must be private or public.');
    return value;
  }

  async function upload(file, options) {
    const chosen = options?.visibility === undefined ? undefined : visibility(options.visibility);
    const bytes = new Uint8Array(await file.arrayBuffer());
    const transfer = await call('transfer_begin', { size: bytes.length, sha256: await checksum(bytes) });
    try {
      for (let offset = 0; offset < bytes.length; offset += transfer.chunkBytes) {
        await call('transfer_write', { transferId: transfer.transferId, offset, data: base64(bytes.subarray(offset, offset + transfer.chunkBytes)) });
      }
      await call('transfer_complete', { transferId: transfer.transferId });
      const saved = { transferId: transfer.transferId, name: file.name, type: file.type || 'application/octet-stream', visibility: chosen };
      return download(await call('files_upload', saved));
    } finally { await call('transfer_cancel', { transferId: transfer.transferId }).catch(() => {}); }
  }

  window.miniapp = {
    actions: { call: async (name, input = {}) => {
      try { return await call('actions_invoke', { name, input }); }
      catch (error) { if (error.status === 401) location.assign(base + '/_konae/login'); throw error; }
    } },
    auth: {
      session: async () => {
        try { return await call('auth_session'); }
        catch (error) { if (error.status === 401) return { userId: 'anonymous', name: 'Anonymous', role: 'anonymous' }; throw error; }
      },
      signIn: () => { location.assign(base + '/_konae/login'); },
      signInWithGoogle: () => { location.assign(base + '/_konae/auth/google/start'); },
      signOut: async () => {
        const result = await call('auth_logout');
        connected = undefined;
        for (const url of urls) URL.revokeObjectURL(url);
        urls.clear();
        return result;
      },
    },
    files: {
      list: async () => {
        const result = [];
        for (const file of await call('files_list')) result.push(await download(file));
        return result;
      },
      upload,
      // The Kernel serves the file on the app's own origin, which connect-src
      // 'self' and img-src 'self' allow; a Blob URL cannot be fetched again.
      url: async (id) => base + '/_konae/files/' + encodeURIComponent(fileId(id)),
      update: async (id, options) => call('files_update', { fileId: fileId(id), visibility: visibility(options && options.visibility) }),
      delete: async (id) => call('files_delete', { fileId: fileId(id) }),
    },
  };
  addEventListener('pagehide', () => { for (const url of urls) URL.revokeObjectURL(url); urls.clear(); });
})();
