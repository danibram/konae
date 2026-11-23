export function dispatch(call, read, writeResponse, failure) {
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const freeze = Object.freeze;
  const encoder = new TextEncoder();
  const encode = encoder.encode.bind(encoder);

  function reject(code) {
    failure(code, null);
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
        else controller.enqueue(chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk));
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
    let response;
    try {
      response = await handler(request, context);
    } catch (error) {
      const detail = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      failure('handler_error', detail.slice(0, 4096));
      throw error;
    }
    if (!(response instanceof Response)) reject('invalid_response');
    const headers = [...response.headers];
    let headerBytes = 0;
    for (const [name, value] of headers) headerBytes += encode(name).length + encode(value).length;
    if (headers.length > 128 || headerBytes > 16384) reject('response_too_large');

    let total = 0;
    if (response.body) {
      const reader = response.body.getReader();
      let complete = false;
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) {
            complete = true;
            break;
          }
          if (!(value instanceof Uint8Array)) reject('invalid_response');
          total += value.length;
          if (total > responseLimit) reject('response_too_large');
          for (let offset = 0; offset < value.length; offset += 64 * 1024) {
            if (!writeResponse(value.subarray(offset, Math.min(offset + 64 * 1024, value.length)))) {
              reject('response_too_large');
            }
          }
        }
      } finally {
        if (!complete) await reader.cancel().catch(() => {});
        reader.releaseLock();
      }
    }
    return stringify({ status: response.status, headers, body: '' });
  }

  async function shutdown(handler) {
    await handler(new AbortController().signal);
  }
  return { invoke, shutdown };
}
