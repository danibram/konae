(() => {
  const { core, primordials } = __bootstrap;
  let timerApi;
  let cloneApi;
  for (const [path, names] of [
    ['ext:deno_web/01_dom_exception.js', ['DOMException']],
    ['ext:deno_web/02_event.js', ['Event', 'EventTarget', 'CustomEvent']],
    ['ext:deno_web/00_url.js', ['URL', 'URLSearchParams']],
    ['ext:deno_web/08_text_encoding.js', ['TextEncoder', 'TextDecoder']],
    ['ext:deno_web/06_streams.js', ['ReadableStream', 'WritableStream', 'TransformStream']],
    ['ext:deno_web/03_abort_signal.js', ['AbortController', 'AbortSignal']],
    ['ext:deno_web/15_performance.js', ['performance']],
    ['ext:deno_web/09_file.js', ['Blob', 'File']],
    ['ext:deno_fetch/20_headers.js', ['Headers']],
    ['ext:deno_fetch/21_formdata.js', ['FormData']],
    ['ext:deno_fetch/23_request.js', ['Request']],
    ['ext:deno_fetch/23_response.js', ['Response']],
  ]) {
    const exports = core.loadExtScript(path);
    for (const name of names) globalThis[name] = exports[name];
  }
  timerApi = core.loadExtScript('ext:deno_web/02_timers.js');
  cloneApi = core.loadExtScript('ext:deno_web/13_message_port.js');
  const cryptoApi = core.loadExtScript('ext:deno_crypto/00_crypto.js');
  const domException = DOMException;
  for (const name of [
    'DataError', 'InvalidAccessError', 'NotSupportedError', 'OperationError', 'QuotaExceededError',
    'SyntaxError', 'TypeMismatchError',
  ]) {
    core.registerErrorBuilder('DOMException' + name, message => new domException(message, name));
  }
  const requestClass = Request;
  const responseClass = Response;
  const controllerClass = AbortController;
  const encode = new TextEncoder().encode.bind(new TextEncoder());
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const decode = decoder.decode.bind(decoder);
  const call = core.ops.op_worker_call;
  const bodyRead = core.ops.op_worker_body_read;
  const failure = core.ops.op_worker_failure;
  const nativeQueueMicrotask = primordials.queueMicrotask
    ?? ((callback) => Promise.resolve().then(callback));
  const transferArrayBuffer = ArrayBuffer.prototype.transfer;
  const timerIds = new Set();
  const stringify = JSON.stringify;
  let controller;
  let requestActive = false;
  let retirementActive = false;
  const NativeDate = Date;
  const nativeNow = Date.now;
  const currentTime = () => requestActive || retirementActive ? nativeNow() : 0;
  function KonaeDate(...args) {
    if (!new.target) return new NativeDate(currentTime()).toString();
    return args.length ? new NativeDate(...args) : new NativeDate(currentTime());
  }
  KonaeDate.prototype = NativeDate.prototype;
  Object.setPrototypeOf(KonaeDate, NativeDate);
  KonaeDate.now = currentTime;
  globalThis.Date = KonaeDate;
  globalThis.fetch = () => reject('unsupported_runtime_api');
  const nativeRandom = Math.random;
  Math.random = () => {
    if (!requestActive && !retirementActive) reject('unsupported_runtime_api');
    return nativeRandom();
  };
  function reject(code) { failure(code); throw new Error(code); }
  function requireRequestEffect() {
    if (!requestActive) reject('unsupported_runtime_api');
  }
  function clearRequestTimers() {
    for (const id of timerIds) {
      Reflect.apply(timerApi.clearTimeout, globalThis, [id]);
      Reflect.apply(timerApi.clearInterval, globalThis, [id]);
    }
    timerIds.clear();
  }
  function timer(setter, callback, delay, args) {
    if (typeof callback !== 'function') throw new TypeError('Timer callback must be a function');
    requireRequestEffect();
    let id;
    const wrapped = (...values) => {
      if (!requestActive) return;
      if (setter === timerApi.setTimeout) timerIds.delete(id);
      return callback(...values);
    };
    id = setter(wrapped, delay, ...args);
    timerIds.add(id);
    return id;
  }
  function clearTimer(clearer, id) {
    requireRequestEffect();
    timerIds.delete(id);
    return clearer(id);
  }
  function requestQueueMicrotask(callback) {
    if (typeof callback !== 'function') throw new TypeError('Callback must be a function');
    requireRequestEffect();
    nativeQueueMicrotask(callback);
  }
  function cloneWithTransfers(value, options) {
    const cloned = cloneApi.structuredClone(value, options);
    for (const transferable of options?.transfer ?? []) {
      if (transferArrayBuffer && transferable instanceof ArrayBuffer) {
        Reflect.apply(transferArrayBuffer, transferable, [0]);
      }
    }
    return cloned;
  }
  function algorithmName(value) {
    return typeof value === 'string' ? value.toUpperCase() : value?.name?.toUpperCase();
  }
  const cryptoHashes = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
  const rsaSignatures = ['RSASSA-PKCS1-V1_5', 'RSA-PSS'];
  const keyFormats = ['jwk', 'spki', 'pkcs8', 'raw'];
  function keyFormat(value) {
    const format = String(value);
    if (!keyFormats.includes(format)) throw new TypeError('Invalid key format.');
    return format;
  }
  function signatureParameters(algorithm) {
    const name = algorithmName(algorithm);
    if (name === 'HMAC' || name === rsaSignatures[0]) return name;
    if (name === 'ECDSA') {
      const hash = algorithmName(algorithm.hash);
      if (cryptoHashes.includes(hash)) return { name, hash };
    }
    if (name === 'RSA-PSS') {
      const saltLength = Math.trunc(+algorithm.saltLength);
      if (!Number.isFinite(saltLength) || saltLength < 0 || saltLength > 0xffffffff) {
        throw new TypeError('Expected an unsigned 32-bit integer.');
      }
      return { name, saltLength };
    }
    throw new DOMException('Unsupported signature algorithm', 'NotSupportedError');
  }
  function makeSubtle() {
    const getSubtle = () => {
      requireRequestEffect();
      return cryptoApi.crypto.subtle;
    };
    const facade = Object.create(null);
    facade.digest = async (algorithm, data) => {
      const subtle = getSubtle();
      if (!cryptoHashes.includes(algorithmName(algorithm))) {
        throw new DOMException('Unsupported digest algorithm', 'NotSupportedError');
      }
      return subtle.digest(algorithm, data);
    };
    facade.importKey = async (format, keyData, algorithm, extractable, usages) => {
      const subtle = getSubtle();
      format = keyFormat(format);
      const name = algorithmName(algorithm);
      const hash = algorithmName(algorithm?.hash);
      const namedCurve = algorithm?.namedCurve;
      const allowed = (name === 'HMAC' && cryptoHashes.includes(hash) && ['raw', 'jwk'].includes(format))
        || (name === 'ECDSA' && namedCurve === 'P-256')
        || (rsaSignatures.includes(name) && cryptoHashes.includes(hash) && format !== 'raw');
      if (!allowed) throw new DOMException('Unsupported key algorithm', 'NotSupportedError');
      return subtle.importKey(format, keyData, { name, hash, namedCurve, length: algorithm?.length }, extractable, usages);
    };
    facade.exportKey = async (format, key) => getSubtle().exportKey(keyFormat(format), key);
    facade.sign = async (algorithm, key, data) => {
      const subtle = getSubtle();
      const parameters = signatureParameters(algorithm);
      if (parameters.name === 'RSA-PSS' && parameters.saltLength > 1024) {
        throw new DOMException('RSA-PSS salt length exceeds the supported size', 'OperationError');
      }
      return subtle.sign(parameters, key, data);
    };
    facade.verify = async (algorithm, key, signature, data) =>
      getSubtle().verify(signatureParameters(algorithm), key, signature, data);
    return Object.freeze(facade);
  }
  let subtleFacade;
  const requestCrypto = Object.freeze({
    getRandomValues(value) { requireRequestEffect(); return cryptoApi.crypto.getRandomValues(value); },
    randomUUID() { requireRequestEffect(); return cryptoApi.crypto.randomUUID(); },
    get subtle() {
      if (!subtleFacade) subtleFacade = makeSubtle();
      return subtleFacade;
    },
  });
  Object.defineProperties(globalThis, {
    setTimeout: { value: (fn, ms, ...args) => timer(timerApi.setTimeout, fn, ms, args), configurable: false },
    setInterval: { value: (fn, ms, ...args) => timer(timerApi.setInterval, fn, ms, args), configurable: false },
    clearTimeout: { value: (id) => clearTimer(timerApi.clearTimeout, id), configurable: false },
    clearInterval: { value: (id) => clearTimer(timerApi.clearInterval, id), configurable: false },
    queueMicrotask: { value: requestQueueMicrotask, configurable: true },
    structuredClone: { value: cloneWithTransfers, configurable: false },
    crypto: { get() { return requestCrypto; }, configurable: false },
    CryptoKey: { value: cryptoApi.CryptoKey, configurable: false },
  });
  Object.defineProperty(AbortSignal, 'timeout', {
    value() { reject('unsupported_runtime_api'); }, writable: false, configurable: false,
  });
  delete globalThis.Deno;
  delete globalThis.__bootstrap;
  return {
    async invoke(handler, input, capability, responseLimit) {
      requestActive = true;
      let bodySequence = 0;
      const body = input.method === 'POST' ? new ReadableStream({
        pull(stream) {
          const chunk = bodyRead(capability, ++bodySequence, 64 * 1024);
          const bytes = new Uint8Array(chunk.bytes);
          if (bytes.byteLength > 0) stream.enqueue(bytes);
          if (chunk.done) stream.close();
        },
        cancel() {},
      }) : undefined;
      const request = new requestClass(input.url, {
        method: input.method,
        headers: input.headers,
        body,
      });
      const context = {
        actor: Object.freeze({ userId: input.actor || null }),
        actions: Object.freeze({
          call: async (name, args = {}) => call(capability, 'action', { name, input: args }),
        }),
      };
      try {
        const response = await handler(request, context);
        if (!(response instanceof responseClass)) reject('invalid_response');
        const headers = [...response.headers];
        let headerBytes = 0;
        for (const [name, value] of headers) headerBytes += encode(name).byteLength + encode(value).byteLength;
        if (headers.length > 128 || headerBytes > 16384) reject('response_too_large');
        const chunks = [];
        let total = 0;
        if (response.body) {
          const reader = response.body.getReader();
          try {
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              total += value.byteLength;
              if (total > responseLimit) { void reader.cancel().catch(() => {}); reject('response_too_large'); }
              chunks.push(value);
            }
          } finally { reader.releaseLock(); }
        }
        const bytes = new Uint8Array(total);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        let body;
        try { body = decode(bytes); } catch { reject('invalid_response'); }
        return stringify({ status: response.status, headers, body });
      } finally {
        requestActive = false;
        clearRequestTimers();
        controller = undefined;
      }
    },
    cancel() {
      requestActive = false;
      clearRequestTimers();
      controller?.abort(new DOMException('Request cancelled', 'AbortError'));
    },
    async shutdown(handler) {
      requestActive = false;
      clearRequestTimers();
      controller = new controllerClass();
      retirementActive = true;
      try { if (typeof handler === 'function') await handler(controller.signal); }
      finally {
        retirementActive = false;
        controller = undefined;
      }
      return 'null';
    },
  };
})()
