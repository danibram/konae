import { opWorkerCall, opWorkerBodyRead, opWorkerResponseChunk, opWorkerFailure, nowMs, randomSeed } from 'konae:worker/host';
import { dispatch } from './dispatch.js';
import { installCrypto } from './starling-crypto.js';

let running = false;
let requestGeneration = 0;
let initializationEffect = false;
const timerRecords = new Map();
const nativeTimers = {
  setTimeout: globalThis.setTimeout,
  clearTimeout: globalThis.clearTimeout,
  setInterval: globalThis.setInterval,
  clearInterval: globalThis.clearInterval,
  queueMicrotask: globalThis.queueMicrotask,
};
const nativePerformance = globalThis.performance;

function refuseInitializationEffect(api) {
  initializationEffect = true;
  throw new TypeError(`KONAE_INIT_EFFECT: ${api}`);
}

function callable(value, api) {
  if (typeof value !== 'function') throw new TypeError(`${api} callback must be a function`);
  return value;
}

function clearTimer(name, handle) {
  const record = timerRecords.get(handle);
  if (record) {
    nativeTimers[record.clear](handle);
    timerRecords.delete(handle);
  } else if (running) {
    nativeTimers[name](handle);
  } else {
    refuseInitializationEffect(name);
  }
}

function scheduleTimer(name, callback, delay, ...args) {
  callback = callable(callback, name);
  if (!running) refuseInitializationEffect(name);
  const generation = requestGeneration;
  const isInterval = name === 'setInterval';
  let handle;
  const wrapped = (...callbackArgs) => {
    if (generation !== requestGeneration || !running) return;
    if (!isInterval) timerRecords.delete(handle);
    callback(...callbackArgs);
  };
  handle = nativeTimers[name](wrapped, delay, ...args);
  timerRecords.set(handle, { clear: isInterval ? 'clearInterval' : 'clearTimeout' });
  return handle;
}

function clearAllTimers() {
  for (const [handle, record] of timerRecords) nativeTimers[record.clear](handle);
  timerRecords.clear();
}

globalThis.fetch = () => {
  if (!running) refuseInitializationEffect('fetch');
  opWorkerFailure('unsupported_runtime_api', null);
  throw new TypeError('unsupported_runtime_api');
};
globalThis.setTimeout = (callback, delay, ...args) => scheduleTimer('setTimeout', callback, delay, ...args);
globalThis.setInterval = (callback, delay, ...args) => scheduleTimer('setInterval', callback, delay, ...args);
globalThis.clearTimeout = handle => clearTimer('clearTimeout', handle);
globalThis.clearInterval = handle => clearTimer('clearInterval', handle);
globalThis.queueMicrotask = callback => {
  callback = callable(callback, 'queueMicrotask');
  if (!running) refuseInitializationEffect('queueMicrotask');
  const generation = requestGeneration;
  nativeTimers.queueMicrotask(() => {
    if (running && generation === requestGeneration) callback();
  });
};

let handlingRequest = false;
installCrypto(api => {
  if (!running) refuseInitializationEffect(api);
  if (!handlingRequest) throw new DOMException('The request has ended.', 'InvalidStateError');
});
delete globalThis.WebAssembly;
const performanceProxy = new Proxy(nativePerformance, {
  get(target, property) {
    if (property === 'now') {
      return () => running ? Reflect.apply(Reflect.get(target, property, target), target, []) : 0;
    }
    const value = Reflect.get(target, property, target);
    return typeof value === 'function' ? value.bind(target) : value;
  },
});
Object.defineProperty(globalThis, 'performance', { value: performanceProxy, configurable: true, writable: true });

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
  if (!running) refuseInitializationEffect('Math.random');
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
      requestGeneration++;
      seedRandom(randomSeed());
      if (typeof app.handle !== 'function') {
        opWorkerFailure('function_handle_missing');
        throw new Error('The bundle must export handle');
      }
      handlingRequest = true;
      try {
        return await runtime.invoke(app.handle, input, limit);
      } finally {
        handlingRequest = false;
        clearAllTimers();
        running = false;
      }
    },
    hasShutdown() { return typeof app.shutdown === 'function'; },
    async shutdown() {
      running = true;
      requestGeneration++;
      seedRandom(randomSeed());
      try {
        if (typeof app.shutdown === 'function') await runtime.shutdown(app.shutdown);
      } finally {
        clearAllTimers();
        running = false;
      }
    },
    spin() { while (true) {} },
  };
}

export function initializationFailed() { return initializationEffect; }
