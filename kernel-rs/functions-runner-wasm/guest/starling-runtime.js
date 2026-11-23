import { opWorkerCall, opWorkerBodyRead, opWorkerResponseChunk, opWorkerFailure, nowMs, randomSeed } from 'konae:worker/host';
import { dispatch } from './dispatch.js';

const forbiddenAtInitialization = "The function's top-level code must finish without pending promises, timers, fetch, or random values.";
let initializationEffect = false;
for (const name of ['fetch', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval']) {
  globalThis[name] = () => {
    if (!running) initializationEffect = true;
    throw new TypeError(`${name} is not available in function handlers`);
  };
}
for (const name of ['crypto', 'performance', 'WebAssembly']) delete globalThis[name];

let running = false;
const NativeDate = Date;
const buildTimestamp = __KONAE_BUILD_TIMESTAMP__;
const currentTime = () => running ? nowMs() : buildTimestamp;
function KonaeDate(...args) {
  if (!new.target) return new NativeDate(currentTime()).toString();
  return args.length ? new NativeDate(...args) : new NativeDate(currentTime());
}
KonaeDate.prototype = NativeDate.prototype;
Object.setPrototypeOf(KonaeDate, NativeDate);
KonaeDate.now = currentTime;
globalThis.Date = KonaeDate;

let randomState = 0;
function seedRandom(seed) {
  const value = BigInt(seed);
  randomState = Number((value ^ (value >> 32n)) & 0xffffffffn) >>> 0;
  if (!randomState) randomState = 0x6d2b79f5;
}
Math.random = () => {
  if (!running) {
    initializationEffect = true;
    throw new TypeError(`function.initialization-cannot-snapshot: ${forbiddenAtInitialization}`);
  }
  randomState ^= randomState << 13;
  randomState ^= randomState >>> 17;
  randomState ^= randomState << 5;
  const high = randomState >>> 5;
  randomState ^= randomState << 13;
  randomState ^= randomState >>> 17;
  randomState ^= randomState << 5;
  const low = randomState >>> 6;
  return (high * 67108864 + low) / 9007199254740992;
};

const runtime = dispatch(opWorkerCall, () => {
  const bytes = opWorkerBodyRead();
  return bytes.length ? bytes : null;
}, bytes => opWorkerResponseChunk(bytes), opWorkerFailure);

export function hostRuntime(app) {
  return {
    async handle(input, limit) {
      running = true;
      seedRandom(randomSeed());
      for (const name of ['fetch', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval']) {
        delete globalThis[name];
      }
      if (typeof app.handle !== 'function') {
        opWorkerFailure('function_handle_missing');
        throw new Error('The bundle must export handle');
      }
      return await runtime.invoke(app.handle, input, limit);
    },
    hasShutdown() { return typeof app.shutdown === 'function'; },
    async shutdown() {
      running = true;
      seedRandom(randomSeed());
      if (typeof app.shutdown === 'function') await runtime.shutdown(app.shutdown);
    },
    spin() { while (true) {} },
  };
}

export function initializationFailed() { return initializationEffect; }
