import { opWorkerCall, opWorkerBodyRead, opWorkerResponseChunk, opWorkerFailure, nowMs } from 'konae:worker/host';
import { dispatch } from './dispatch.js';

for (const name of [
  'fetch', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval',
  'queueMicrotask', 'crypto', 'performance', 'WebAssembly',
]) delete globalThis[name];

let running = false;
Date.now = () => running ? nowMs() : 0;

const runtime = dispatch(opWorkerCall, () => {
  const bytes = opWorkerBodyRead();
  return bytes.length ? bytes : null;
}, bytes => opWorkerResponseChunk(bytes), opWorkerFailure);

export function hostRuntime(app) {
  return {
    async handle(input, limit) {
      running = true;
      if (typeof app.handle !== 'function') {
        opWorkerFailure('function_handle_missing');
        throw new Error('The bundle must export handle');
      }
      return await runtime.invoke(app.handle, input, limit);
    },
    hasShutdown() { return typeof app.shutdown === 'function'; },
    async shutdown() {
      running = true;
      if (typeof app.shutdown === 'function') await runtime.shutdown(app.shutdown);
    },
    spin() { while (true) {} },
  };
}
