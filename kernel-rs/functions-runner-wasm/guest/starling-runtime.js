import { opWorkerCall, opWorkerBodyRead, opWorkerResponseChunk, opWorkerFailure, nowMs, randomSeed } from 'konae:worker/host';
import { dispatch } from './dispatch.js';

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
const nativeCrypto = globalThis.crypto;
const nativePerformance = globalThis.performance;
const initEffectCode = 'function.wasm-init-effect';

function refuseInitializationEffect(api) {
  if (!running) {
    initializationEffect = true;
    throw new TypeError(`${initEffectCode}: A function cannot start timers or fetch during top-level initialization.`);
  }
  throw new TypeError(`${api} is unavailable in this function runtime`);
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

globalThis.fetch = (...args) => refuseInitializationEffect('fetch');
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

function cryptoCall(target, property) {
  const method = Reflect.get(target, property, target);
  if (typeof method !== 'function') return method;
  return (...args) => {
    if (!running) refuseInitializationEffect(`crypto.${String(property)}`);
    const result = Reflect.apply(method, target, args);
    if (property !== 'importKey' || args[0] !== 'jwk' || args[3] !== true) return result;
    const jwk = args[1];
    if (!jwk || jwk.ext !== undefined || jwk.d !== undefined || !['EC', 'RSA'].includes(jwk.kty)) return result;
    return result.catch(error => {
      if (error?.name !== 'DataError') throw error;
      const compatibleArgs = [...args];
      compatibleArgs[3] = false;
      return Reflect.apply(method, target, compatibleArgs);
    });
  };
}

const subtleProxy = new Proxy(nativeCrypto.subtle, {
  get(target, property) { return cryptoCall(target, property); },
});
const cryptoProxy = new Proxy(nativeCrypto, {
  get(target, property) {
    if (property === 'subtle') return subtleProxy;
    return cryptoCall(target, property);
  },
});
Object.defineProperty(globalThis, 'crypto', { value: cryptoProxy, configurable: true, writable: true });
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
  if (!running) {
    initializationEffect = true;
    throw new TypeError(`${initEffectCode}: A function cannot use random values during top-level initialization.`);
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
      requestGeneration++;
      seedRandom(randomSeed());
      if (typeof app.handle !== 'function') {
        opWorkerFailure('function_handle_missing');
        throw new Error('The bundle must export handle');
      }
      try {
        return await runtime.invoke(app.handle, input, limit);
      } finally {
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
