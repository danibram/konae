import { TextDecoder, TextEncoder } from './text.js';
import { AbortSignal } from './abort.js';
import { ReadableStream } from 'web-streams-polyfill';
import { URL, URLSearchParams } from 'whatwg-url-without-unicode';

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const bytesOf = (value) => ArrayBuffer.isView(value)
  ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
  : value instanceof ArrayBuffer ? new Uint8Array(value) : encoder.encode(String(value));

function concat(chunks) {
  const bytes = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
}

function byteStream(bytes) {
  return new ReadableStream({
    start(controller) {
      if (bytes.length) controller.enqueue(bytes);
      controller.close();
    },
  });
}

function headerName(value) {
  const name = String(value).toLowerCase();
  if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name)) throw new TypeError('Invalid header name');
  return name;
}

export class Headers {
  #values = new Map();

  constructor(init = []) {
    const entries = init[Symbol.iterator] ? init : Object.entries(init);
    for (const [name, value] of entries) this.append(name, value);
  }

  append(name, value) {
    name = headerName(name);
    value = String(value).replace(/^[\t ]+|[\t ]+$/g, '');
    if (/[\u0000\r\n]/.test(value)) throw new TypeError('Invalid header value');
    const values = this.#values.get(name);
    if (values) values.push(value);
    else this.#values.set(name, [value]);
  }

  set(name, value) { this.delete(name); this.append(name, value); }
  delete(name) { this.#values.delete(headerName(name)); }
  has(name) { return this.#values.has(headerName(name)); }
  get(name) { return this.#values.get(headerName(name))?.join(', ') ?? null; }
  getSetCookie() { return [...(this.#values.get('set-cookie') ?? [])]; }
  *entries() {
    for (const name of [...this.#values.keys()].sort()) {
      const values = this.#values.get(name);
      if (name === 'set-cookie') {
        for (const value of values) yield [name, value];
      } else yield [name, values.join(', ')];
    }
  }
  *keys() { for (const [name] of this) yield name; }
  *values() { for (const [, value] of this) yield value; }
  [Symbol.iterator]() { return this.entries(); }
  forEach(callback, that) { for (const [key, value] of this) callback.call(that, value, key, this); }
}

export class Blob {
  #bytes;

  constructor(parts = [], { type = '' } = {}) {
    this.#bytes = concat(Array.from(parts, (part) => part instanceof Blob ? part.#bytes : bytesOf(part)));
    this.type = /^[\x20-\x7e]*$/.test(type) ? type.toLowerCase() : '';
  }
  get size() { return this.#bytes.length; }
  async bytes() { return this.#bytes.slice(); }
  async arrayBuffer() { return this.#bytes.slice().buffer; }
  async text() { return decoder.decode(this.#bytes); }
  stream() { return byteStream(this.#bytes.slice()); }
  slice(start, end, type) { return new Blob([this.#bytes.subarray(start, end)], { type }); }
}

export class File extends Blob {
  constructor(parts, name, options = {}) {
    super(parts, options);
    this.name = String(name);
    this.lastModified = options.lastModified ?? Date.now();
  }
}

export class FormData {
  #entries = [];

  #value(value, filename) {
    if (!(value instanceof Blob)) return String(value);
    if (value instanceof File && filename === undefined) return value;
    return new File([value], filename ?? 'blob', { type: value.type });
  }
  append(name, value, filename) { this.#entries.push([String(name), this.#value(value, filename)]); }
  delete(name) { this.#entries = this.#entries.filter(([key]) => key !== String(name)); }
  get(name) { return this.#entries.find(([key]) => key === String(name))?.[1] ?? null; }
  getAll(name) { return this.#entries.filter(([key]) => key === String(name)).map(([, value]) => value); }
  has(name) { return this.get(name) !== null; }
  set(name, value, filename) {
    name = String(name);
    const index = this.#entries.findIndex(([key]) => key === name);
    this.delete(name);
    this.#entries.splice(index < 0 ? this.#entries.length : index, 0, [name, this.#value(value, filename)]);
  }
  *entries() { for (const entry of this.#entries) yield [...entry]; }
  *keys() { for (const [name] of this) yield name; }
  *values() { for (const [, value] of this) yield value; }
  forEach(callback, that) { for (const [name, value] of this) callback.call(that, value, name, this); }
  [Symbol.iterator]() { return this.entries(); }
}

class Body {
  #stream;
  #used = false;

  constructor(value, init) {
    this.headers = new Headers(init.headers);
    if (value == null) this.#stream = null;
    else if (value instanceof ReadableStream) this.#stream = value;
    else if (value instanceof Blob) {
      this.#stream = value.stream();
      if (value.type && !this.headers.has('content-type')) this.headers.set('content-type', value.type);
    } else if (value instanceof FormData) {
      const boundary = 'konae-' + Math.random().toString(36).slice(2);
      const escape = (token) => token.replaceAll('"', '%22').replaceAll('\r', '%0D').replaceAll('\n', '%0A');
      const parts = [];
      for (const [name, entry] of value) {
        let header = `--${boundary}\r\nContent-Disposition: form-data; name="${escape(name)}"`;
        if (entry instanceof File) {
          header += `; filename="${escape(entry.name)}"\r\nContent-Type: ${entry.type || 'application/octet-stream'}`;
        }
        parts.push(header + '\r\n\r\n', entry, '\r\n');
      }
      parts.push(`--${boundary}--\r\n`);
      this.#stream = new Blob(parts).stream();
      if (!this.headers.has('content-type')) this.headers.set('content-type', `multipart/form-data; boundary=${boundary}`);
    } else {
      const type = value instanceof URLSearchParams ? 'application/x-www-form-urlencoded;charset=UTF-8'
        : typeof value === 'string' ? 'text/plain;charset=UTF-8' : null;
      if (type && !this.headers.has('content-type')) this.headers.set('content-type', type);
      this.#stream = byteStream(bytesOf(value).slice());
    }
  }

  get body() { return this.#stream; }
  get bodyUsed() { return this.#used; }

  fork() {
    if (this.#used || this.#stream?.locked) throw new TypeError('Body already used');
    if (!this.#stream) return null;
    const [current, copy] = this.#stream.tee();
    this.#stream = current;
    return copy;
  }

  async bytes() {
    if (!this.#stream) return new Uint8Array();
    if (this.#used) throw new TypeError('Body already used');
    const reader = this.#stream.getReader();
    this.#used = true;
    const chunks = [];
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return concat(chunks);
        if (!(value instanceof Uint8Array)) throw new TypeError('Invalid body chunk');
        chunks.push(value);
      }
    } finally { reader.releaseLock(); }
  }

  async arrayBuffer() { return (await this.bytes()).buffer; }
  async text() { return decoder.decode(await this.bytes()); }
  async json() { return JSON.parse(await this.text()); }
  async blob() { return new Blob([await this.bytes()], { type: this.headers.get('content-type') ?? '' }); }
  async formData() {
    const type = this.headers.get('content-type') ?? '';
    const result = new FormData();
    if (type.split(';')[0].toLowerCase() === 'application/x-www-form-urlencoded') {
      for (const [name, value] of new URLSearchParams(await this.text())) result.append(name, value);
      return result;
    }
    const boundary = /^multipart\/form-data;\s*boundary=(?:"([^"]+)"|([^; ]+))/i.exec(type);
    if (!boundary) throw new TypeError('Expected form data');
    const raw = await this.bytes();
    const chunks = [];
    for (let i = 0; i < raw.length; i += 4096) chunks.push(String.fromCharCode(...raw.subarray(i, i + 4096)));
    const marker = '--' + (boundary[1] ?? boundary[2]);
    const source = chunks.join('');
    if (!source.startsWith(marker)) throw new TypeError('Missing multipart boundary');
    let offset = marker.length;
    while (source.slice(offset, offset + 2) === '\r\n') {
      const headerEnd = source.indexOf('\r\n\r\n', offset + 2);
      const partEnd = source.indexOf('\r\n' + marker, headerEnd + 4);
      if (headerEnd < 0 || partEnd < 0) throw new TypeError('Incomplete multipart body');
      const header = decoder.decode(raw.subarray(offset + 2, headerEnd));
      const disposition = /^content-disposition: form-data; name="([^"]*)"(?:; filename="([^"]*)")?/im.exec(header);
      if (!disposition) throw new TypeError('Missing form field name');
      const part = raw.subarray(headerEnd + 4, partEnd);
      const fileType = /^content-type:\s*([^\r\n]+)/im.exec(header)?.[1];
      result.append(disposition[1], disposition[2] === undefined ? decoder.decode(part)
        : new File([part], disposition[2], { type: fileType }));
      offset = partEnd + 2 + marker.length;
    }
    if (source.slice(offset, offset + 2) !== '--') throw new TypeError('Missing closing form boundary');
    return result;
  }
}

export class Request extends Body {
  constructor(input, init = {}) {
    const other = input instanceof Request ? input : null;
    const method = String(init.method ?? other?.method ?? 'GET');
    const body = 'body' in init ? init.body : other?.fork();
    if (/^(GET|HEAD)$/i.test(method) && body != null) throw new TypeError('Method forbids a body');
    super(body, { headers: init.headers ?? other?.headers });
    this.url = other?.url ?? new URL(input).href;
    this.method = /^(DELETE|GET|HEAD|OPTIONS|POST|PUT)$/i.test(method) ? method.toUpperCase() : method;
    this.signal = init.signal ?? other?.signal ?? new AbortSignal();
  }
  clone() { return new Request(this); }
}

export class Response extends Body {
  constructor(body = null, init = {}) {
    const status = Number(init.status ?? 200);
    if (!Number.isInteger(status) || status < 200 || status > 599) throw new RangeError('Invalid response status');
    if ([204, 205, 304].includes(status) && body != null) throw new TypeError('Status forbids a body');
    super(body, init);
    this.status = status;
    this.statusText = init.statusText ?? '';
  }
  get ok() { return this.status >= 200 && this.status < 300; }
  clone() { return new Response(this.fork(), this); }
  static json(value, init = {}) {
    const headers = new Headers(init.headers);
    if (!headers.has('content-type')) headers.set('content-type', 'application/json');
    return new Response(JSON.stringify(value), { ...init, headers });
  }
  static redirect(url, status = 302) {
    if (![301, 302, 303, 307, 308].includes(status)) throw new RangeError('Invalid redirect status');
    return new Response(null, { status, headers: { location: new URL(url).href } });
  }
}
