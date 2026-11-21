(() => {
  const { core } = __bootstrap;
  for (const [path, names] of [
    ['ext:deno_web/01_dom_exception.js', ['DOMException']],
    ['ext:deno_web/02_event.js', ['Event', 'EventTarget', 'CustomEvent']],
    ['ext:deno_web/00_url.js', ['URL', 'URLSearchParams']],
    ['ext:deno_web/08_text_encoding.js', ['TextEncoder', 'TextDecoder']],
    ['ext:deno_web/06_streams.js', ['ReadableStream', 'WritableStream', 'TransformStream']],
    ['ext:deno_web/03_abort_signal.js', ['AbortController', 'AbortSignal']],
    ['ext:deno_web/09_file.js', ['Blob', 'File']],
    ['ext:deno_fetch/20_headers.js', ['Headers']],
    ['ext:deno_fetch/21_formdata.js', ['FormData']],
    ['ext:deno_fetch/23_request.js', ['Request']],
    ['ext:deno_fetch/23_response.js', ['Response']],
  ]) {
    const exports = core.loadExtScript(path);
    for (const name of names) globalThis[name] = exports[name];
  }
  const requestClass = Request;
  const controllerClass = AbortController;
  const encode = new TextEncoder().encode.bind(new TextEncoder());
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const decode = decoder.decode.bind(decoder);
  const call = core.ops.op_worker_call;
  const bodyRead = core.ops.op_worker_body_read;
  const failure = core.ops.op_worker_failure;
  const stringify = JSON.stringify;
  let controller;
  function reject(code) { failure(code); throw new Error(code); }
  Object.defineProperty(AbortSignal, 'timeout', {
    value() { reject('unsupported_runtime_api'); }, writable: false, configurable: false,
  });
  delete globalThis.Deno;
  delete globalThis.__bootstrap;
  return {
    async invoke(handler, input, capability, responseLimit) {
      controller = new controllerClass();
      let bodySequence = 0;
      const body = new ReadableStream({
        pull(stream) {
          const chunk = bodyRead(capability, ++bodySequence, 64 * 1024);
          const bytes = new Uint8Array(chunk.bytes);
          if (bytes.byteLength > 0) stream.enqueue(bytes);
          if (chunk.done) stream.close();
        },
        cancel() {},
      });
      const request = new requestClass(input.url, {
        method: input.method,
        headers: input.headers,
        signal: controller.signal,
        ...(input.method === 'POST' ? { body } : {}),
      });
      const invoke = (operation, args) => call(capability, operation, args);
      const context = {
        actor: Object.freeze({ userId: input.actor || null }),
        call: invoke,
        actions: Object.freeze({ call: async (name, args = {}) => invoke('action', { name, input: args }) }),
        data: Object.freeze({
          readOrder: (id) => invoke('readOrder', { id }),
          updateOrder: (id, quantity) => invoke('updateOrder', { id, quantity }),
        }),
      };
      try {
        const response = await handler(request, context);
        const headers = [...response.headers];
        let headerBytes = 0;
        for (const [name, value] of headers) headerBytes += encode(name).byteLength + encode(value).byteLength;
        if (headers.length > 128 || headerBytes > 16384) reject('response_too_large');
        const chunks = [];
        let total = 0;
        if (response.body) {
          const reader = response.body.getReader();
          try {
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              total += value.byteLength;
              if (total > responseLimit) { void reader.cancel().catch(() => {}); reject('response_too_large'); }
              chunks.push(value);
            }
          } finally { reader.releaseLock(); }
        }
        const bytes = new Uint8Array(total);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        let body;
        try { body = decode(bytes); } catch { reject('invalid_response'); }
        return stringify({ status: response.status, headers, body });
      } finally { controller = undefined; }
    },
    cancel() { controller?.abort(new DOMException('Request cancelled', 'AbortError')); },
    async shutdown(handler) {
      controller = new controllerClass();
      try { if (typeof handler === 'function') await handler(controller.signal); }
      finally { controller = undefined; }
      return 'null';
    },
  };
})()
