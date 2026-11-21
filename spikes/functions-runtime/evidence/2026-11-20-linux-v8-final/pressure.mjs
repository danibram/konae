const marker = 'attached';
let calls = 0;
let retained;

export function handle(request) {
  if (request.url.endsWith('/pressure')) {
    retained = new Uint8Array(320 * 1024 * 1024).fill(7);
    return new Response(String(retained[retained.length - 1]));
  }
  return Response.json({ marker, calls: ++calls, deno: typeof globalThis.Deno,
    fetch: typeof globalThis.fetch });
}
