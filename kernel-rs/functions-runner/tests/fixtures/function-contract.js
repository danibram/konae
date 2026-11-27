const initializedAt = Date.now();
const timeout = AbortSignal.timeout;

export function handle(request) {
  switch (new URL(request.url).pathname) {
    case '/fetch':
      try { fetch('https://example.invalid'); } catch {}
      return new Response('unexpected success');
    case '/timeout':
      try { timeout(1); } catch {}
      return new Response('unexpected success');
    case '/invalid-response':
      return { status: 200, headers: new Headers(), body: null };
    default: {
      const controller = new AbortController();
      controller.abort('fixture');
      return Response.json({
        initializedAt,
        fetch: typeof fetch,
        wasm: typeof WebAssembly,
        requestSignal: request.signal instanceof AbortSignal && !request.signal.aborted,
        appSignal: controller.signal.reason,
      });
    }
  }
}
