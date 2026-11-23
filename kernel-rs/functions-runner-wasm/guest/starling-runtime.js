import { opWorkerCall, opWorkerBodyRead, opWorkerFailure, nowMs } from 'konae:worker/host';
import { dispatch } from './dispatch.js';

for (const name of [
  'fetch', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval',
  'queueMicrotask', 'crypto', 'performance', 'WebAssembly',
]) delete globalThis[name];

let running = false;
let responseLimit = 256 * 1024;
Date.now = () => running ? nowMs() : 0;

const NativeResponse = globalThis.Response;
globalThis.Response = new Proxy(NativeResponse, {
  construct(target, args, newTarget) {
    const body = args[0];
    let size;
    if (typeof body === 'string') {
      size = 0;
      for (const character of body) {
        const point = character.codePointAt(0);
        size += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
        if (size > responseLimit) break;
      }
    } else if (ArrayBuffer.isView(body)) {
      size = body.byteLength;
    } else if (body instanceof ArrayBuffer) {
      size = body.byteLength;
    } else if (body instanceof Blob) {
      size = body.size;
    }
    if (size > responseLimit) {
      opWorkerFailure('response_too_large', null);
      throw new Error('response_too_large');
    }
    return Reflect.construct(target, args, newTarget);
  },
});

const runtime = dispatch(opWorkerCall, () => {
  const bytes = opWorkerBodyRead();
  return bytes.length ? bytes : null;
}, opWorkerFailure);

export function hostRuntime(app) {
  return {
    async handle(input, limit) {
      running = true;
      responseLimit = Math.min(limit, 256 * 1024);
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
