export function dispatch(call, read, failure) {
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const freeze = Object.freeze;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const encode = encoder.encode.bind(encoder);
  const decode = decoder.decode.bind(decoder);

  function reject(code) {
    failure(code);
    throw new Error(code);
  }

  Object.defineProperty(AbortSignal, 'timeout', {
    value() { reject('unsupported_runtime_api'); }, writable: false, configurable: false,
  });

  async function invoke(handler, encodedInput, responseLimit) {
    const input = parse(encodedInput);
    const body = input.method === 'POST' ? new ReadableStream({
      pull(controller) {
        const chunk = read();
        if (chunk === null) controller.close();
        else controller.enqueue(new Uint8Array(chunk));
      },
    }, { highWaterMark: 0 }) : null;
    const request = new Request(input.url, { method: input.method, headers: input.headers, body, duplex: 'half' });
    const context = {
      actor: freeze({ userId: input.actor || null }),
      actions: freeze({
        async call(name, args = {}) {
          const result = parse(call(stringify({ name, input: args })));
          if ('Err' in result) throw new Error(result.Err);
          return result.Ok;
        },
      }),
    };
    const response = await handler(request, context);
    if (!(response instanceof Response)) reject('invalid_response');
    const headers = [...response.headers];
    let headerBytes = 0;
    for (const [name, value] of headers) headerBytes += encode(name).length + encode(value).length;
    if (headers.length > 128 || headerBytes > 16384) reject('response_too_large');

    const chunks = [];
    let total = 0;
    if (response.body) {
      const reader = response.body.getReader();
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (!(value instanceof Uint8Array)) reject('invalid_response');
          total += value.length;
          if (total > responseLimit) reject('response_too_large');
          chunks.push(value);
        }
      } finally {
        reader.releaseLock();
      }
    }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    let decoded;
    try { decoded = decode(bytes); } catch { reject('invalid_response'); }
    return stringify({ status: response.status, headers, body: decoded });
  }

  async function shutdown(handler) {
    await handler(new AbortController().signal);
  }
  return { invoke, shutdown };
}
