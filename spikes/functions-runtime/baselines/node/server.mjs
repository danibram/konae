import { serve } from '@hono/node-server';
import { handle as orders } from '../../app/dist/orders.js';
import { handle as preferences } from '../../app/dist/preferences.js';
import { createHost } from '../lib/host.mjs';

const host = createHost({ directory: process.env.DATA_DIR ?? 'data/node', orders, preferences });
const server = serve({ hostname: '127.0.0.1', port: Number(process.env.PORT ?? 43103), fetch: host.fetch });
function stop() {
  server.close(() => { host.close(); process.exit(0); });
  server.closeAllConnections();
}
process.once('SIGINT', stop);
process.once('SIGTERM', stop);
