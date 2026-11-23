import { Buffer } from "node:buffer";
import { hmac } from "@noble/hashes/hmac";
import { sha1 } from "@noble/hashes/sha1";
import { sha256, sha384, sha512 } from "@noble/hashes/sha2";

const algorithms = { sha1, sha256, sha384, sha512 };

function algorithm(name) {
  const hash = algorithms[name.toLowerCase()];
  if (!Object.hasOwn(algorithms, name.toLowerCase())) {
    throw Object.assign(new Error(`Konae does not support the hash algorithm ${name}.`), {
      code: "ERR_KONAE_UNSUPPORTED_NODE_API",
    });
  }
  return hash;
}

function digest(state) {
  return {
    update(value, encoding) {
      state.update(typeof value === "string" ? Buffer.from(value, encoding) : value);
      return this;
    },
    digest(encoding) {
      const result = Buffer.from(state.digest());
      return encoding === undefined ? result : result.toString(encoding);
    },
  };
}

export const createHash = (name) => digest(algorithm(name).create());
export const createHmac = (name, key) => digest(hmac.create(algorithm(name), Buffer.from(key)));
export function randomBytes(size, callback) {
  const bytes = Buffer.alloc(size);
  for (let i = 0; i < size; i += 65536) crypto.getRandomValues(bytes.subarray(i, i + 65536));
  if (callback === undefined) return bytes;
  queueMicrotask(() => callback(null, bytes));
}
export const randomUUID = () => crypto.randomUUID();
export const getHashes = () => Object.keys(algorithms);
export default { createHash, createHmac, randomBytes, randomUUID, getHashes };
