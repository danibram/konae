import { Hono } from 'hono';
import { cache } from 'hono/cache';
import { setSignedCookie } from 'hono/cookie';
import { etag } from 'hono/etag';
import { jwt } from 'hono/jwt';
import { streamText } from 'hono/streaming';

const app = new Hono();
app.onError((error, context) => context.json({ error: String(error) }, 500));
app.get('/jwt', (context, next) => jwt({ secret: 'spike-fixture', alg: 'HS256' })(context, next), context => context.json({ ok: true }));
app.get('/etag', etag(), context => context.text('etag'));
app.get('/etag-custom', etag({ generateDigest: () => new Uint8Array([1, 2, 3]).buffer }), context => context.text('etag'));
app.get('/cache', cache({ cacheName: 'probe', onCacheNotAvailable: false }), context => context.json({ count: 1 }));
app.get('/cookie', async context => {
  await setSignedCookie(context, 'session', 'demo', 'spike-fixture');
  return context.text('signed');
});
app.get('/stream', context => streamText(context, async stream => {
  await stream.write('one');
  await stream.write('two');
}));
export function handle(request) { return app.fetch(request); }
