import { digest, importKey, exportKey, sign, verify, randomBytes } from 'konae:worker/crypto';

const algorithms = ['HMAC', 'ECDSA', 'RSASSA-PKCS1-v1_5', 'RSA-PSS'];
const hashes = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
const formats = ['raw', 'jwk', 'spki', 'pkcs8'];
const integerViews = [
  'Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
  'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array',
];
const typedArrayTag = Object.getOwnPropertyDescriptor(
  Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag,
).get;
const arrayBufferLength = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
const keyRecords = new WeakMap();
const constructorToken = {};
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const hex = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));

function normalized(value, choices) {
  const name = typeof value === 'string' ? value : value?.name;
  if (name === undefined) throw new TypeError('An algorithm name is required.');
  const match = choices.find(item => item.toUpperCase() === String(name).toUpperCase());
  if (match === undefined) throw new DOMException('Unsupported algorithm.', 'NotSupportedError');
  return match;
}

function keyFormat(value) {
  const format = String(value);
  if (!formats.includes(format)) throw new TypeError('Invalid key format.');
  return format;
}

function bytes(value) {
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  arrayBufferLength.call(value);
  return new Uint8Array(value);
}

function uint32(value) {
  const number = Math.trunc(+value);
  if (!Number.isFinite(number) || number < 0 || number > 0xffffffff) {
    throw new TypeError('Expected an unsigned 32-bit integer.');
  }
  return number;
}

function host(call, ...args) {
  try {
    return call(...args);
  } catch (error) {
    if (typeof error.payload === 'string') {
      throw new DOMException('Web Crypto operation failed.', error.payload);
    }
    throw error;
  }
}

function keyRecord(key) {
  const record = keyRecords.get(key);
  if (!record) throw new TypeError('Expected a CryptoKey.');
  return record;
}

class CryptoKey {
  constructor(token, record) {
    if (token !== constructorToken) throw new TypeError('Illegal constructor');
    keyRecords.set(this, record);
  }

  get type() { return keyRecord(this).type; }
  get extractable() { return keyRecord(this).extractable; }
  get algorithm() { return keyRecord(this).algorithm; }
  get usages() { return keyRecord(this).usages; }
  get [Symbol.toStringTag]() { return 'CryptoKey'; }
}

function signatureParameters(params) {
  const algorithm = normalized(params, algorithms);
  const hash = algorithm === 'ECDSA' ? normalized(params.hash, hashes) : '';
  const salt = algorithm === 'RSA-PSS' ? uint32(params.saltLength) : 0;
  return [algorithm, hash, salt];
}

export function installCrypto(guard) {
  class SubtleCrypto {
    constructor(token) {
      if (token !== constructorToken) throw new TypeError('Illegal constructor');
    }

    async digest(algorithm, data) {
      guard('crypto.subtle.digest');
      return host(digest, normalized(algorithm, hashes), bytes(data)).buffer;
    }

    async importKey(format, material, params, extractable, usages) {
      guard('crypto.subtle.importKey');
      format = keyFormat(format);
      const algorithm = normalized(params, algorithms);
      const options = { format, algorithm, extractable: Boolean(extractable), usages: [...usages].map(String) };
      if (algorithm === 'ECDSA') {
        if (params.namedCurve === undefined) throw new TypeError('A named curve is required.');
        options.namedCurve = String(params.namedCurve);
      } else {
        options.hash = normalized(params.hash, hashes);
        if (algorithm === 'HMAC' && params.length !== undefined) options.length = uint32(params.length);
      }
      const keyData = format === 'jwk' ? encoder.encode(JSON.stringify(material)) : bytes(material);
      const record = JSON.parse(host(importKey, JSON.stringify(options), keyData));
      if (record.algorithm.publicExponent) {
        record.algorithm.publicExponent = Uint8Array.from(record.algorithm.publicExponent);
      }
      return new CryptoKey(constructorToken, record);
    }

    async exportKey(format, key) {
      guard('crypto.subtle.exportKey');
      format = keyFormat(format);
      const material = host(exportKey, keyRecord(key).handle, format);
      return format === 'jwk' ? JSON.parse(decoder.decode(material)) : material.buffer;
    }

    async sign(algorithm, key, data) {
      guard('crypto.subtle.sign');
      const params = signatureParameters(algorithm);
      return host(sign, keyRecord(key).handle, ...params, bytes(data)).buffer;
    }

    async verify(algorithm, key, signature, data) {
      guard('crypto.subtle.verify');
      const params = signatureParameters(algorithm);
      return host(verify, keyRecord(key).handle, ...params, bytes(data), bytes(signature));
    }

    get [Symbol.toStringTag]() { return 'SubtleCrypto'; }
  }

  const subtle = new SubtleCrypto(constructorToken);

  class Crypto {
    constructor(token) {
      if (token !== constructorToken) throw new TypeError('Illegal constructor');
    }

    get subtle() { return subtle; }

    getRandomValues(value) {
      guard('crypto.getRandomValues');
      if (!integerViews.includes(typedArrayTag.call(value))) {
        throw new DOMException('Expected an integer typed array.', 'TypeMismatchError');
      }
      const output = bytes(value);
      if (output.length > 65536) {
        throw new DOMException('Requested more than 65536 bytes.', 'QuotaExceededError');
      }
      output.set(host(randomBytes, output.length));
      return value;
    }

    randomUUID() {
      guard('crypto.randomUUID');
      const value = host(randomBytes, 16);
      value[6] = (value[6] & 15) | 64;
      value[8] = (value[8] & 63) | 128;
      const text = Array.from(value, number => hex[number]).join('');
      return [text.slice(0, 8), text.slice(8, 12), text.slice(12, 16),
        text.slice(16, 20), text.slice(20)].join('-');
    }

    get [Symbol.toStringTag]() { return 'Crypto'; }
  }

  for (const [name, value] of Object.entries({
    crypto: new Crypto(constructorToken), Crypto, SubtleCrypto, CryptoKey,
  })) {
    Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  }
}
