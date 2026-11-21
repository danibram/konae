import { createHost } from '../lib/host.mjs';
import { orders, preferences } from './handlers.mjs';

const key = Symbol.for('konae.functions.baseline.next.host');
export async function dispatch(request) {
  globalThis[key] ??= createHost({ directory: process.env.DATA_DIR ?? 'data/next', orders, preferences });
  return globalThis[key].fetch(request);
}
