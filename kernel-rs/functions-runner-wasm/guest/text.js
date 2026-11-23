export class TextEncoder {
  get encoding() { return 'utf-8'; }

  encode(value = '') {
    const text = String(value).toWellFormed();
    const bytes = new Uint8Array(text.length * 3);
    let length = 0;
    for (const character of text) {
      const point = character.codePointAt(0);
      if (point < 0x80) {
        bytes[length++] = point;
      } else if (point < 0x800) {
        bytes[length++] = 0xc0 | (point >> 6);
        bytes[length++] = 0x80 | (point & 63);
      } else if (point < 0x10000) {
        bytes[length++] = 0xe0 | (point >> 12);
        bytes[length++] = 0x80 | ((point >> 6) & 63);
        bytes[length++] = 0x80 | (point & 63);
      } else {
        bytes[length++] = 0xf0 | (point >> 18);
        bytes[length++] = 0x80 | ((point >> 12) & 63);
        bytes[length++] = 0x80 | ((point >> 6) & 63);
        bytes[length++] = 0x80 | (point & 63);
      }
    }
    return bytes.slice(0, length);
  }

  encodeInto(input, destination) {
    let read = 0;
    let written = 0;
    for (const character of String(input)) {
      const bytes = this.encode(character);
      if (written + bytes.length > destination.length) break;
      destination.set(bytes, written);
      written += bytes.length;
      read += character.length;
    }
    return { read, written };
  }
}

export class TextDecoder {
  #pending = new Uint8Array(0);
  #start = true;

  constructor(label = 'utf-8', { fatal = false, ignoreBOM = false } = {}) {
    if (!['utf-8', 'utf8', 'unicode-1-1-utf-8'].includes(String(label).trim().toLowerCase())) {
      throw new RangeError('Only UTF-8 is available');
    }
    this.fatal = Boolean(fatal);
    this.ignoreBOM = Boolean(ignoreBOM);
  }

  get encoding() { return 'utf-8'; }

  decode(input = new Uint8Array(), { stream = false } = {}) {
    let bytes = ArrayBuffer.isView(input)
      ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
      : new Uint8Array(input);
    if (this.#pending.length) {
      const merged = new Uint8Array(this.#pending.length + bytes.length);
      merged.set(this.#pending);
      merged.set(bytes, this.#pending.length);
      bytes = merged;
    }
    this.#pending = new Uint8Array();
    const chunks = [];
    let characters = '';
    let i = 0;
    while (i < bytes.length) {
      const start = i;
      const first = bytes[i++];
      let point = first;
      let remaining = 0;
      let valid = first < 0x80 || (first >= 0xc2 && first <= 0xf4);
      if (valid && first >= 0x80) {
        remaining = first < 0xe0 ? 1 : first < 0xf0 ? 2 : 3;
        point = first & (0x7f >> (remaining + 1));
        for (let j = 0; j < remaining; j++) {
          if (i === bytes.length && stream) {
            this.#pending = bytes.slice(start);
            break;
          }
          const next = bytes[i];
          const lower = j === 0 && first === 0xe0 ? 0xa0 : j === 0 && first === 0xf0 ? 0x90 : 0x80;
          const upper = j === 0 && first === 0xed ? 0x9f : j === 0 && first === 0xf4 ? 0x8f : 0xbf;
          if (next === undefined || next < lower || next > upper) {
            valid = false;
            break;
          }
          point = (point << 6) | (next & 63);
          i++;
        }
      }
      if (this.#pending.length) break;
      if (!valid && this.fatal) throw new TypeError('Invalid UTF-8');
      if (!(this.#start && !this.ignoreBOM && valid && point === 0xfeff)) {
        characters += valid ? String.fromCodePoint(point) : '\uFFFD';
      }
      this.#start = false;
      if (characters.length >= 8192) {
        chunks.push(characters);
        characters = '';
      }
    }
    if (!stream) this.#start = true;
    chunks.push(characters);
    return chunks.join('');
  }
}
