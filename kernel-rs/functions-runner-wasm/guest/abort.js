export class DOMException extends Error {
  constructor(message = '', name = 'Error') {
    super(message);
    this.name = name;
  }
}

export class AbortSignal {
  #listeners = new Map();
  #reason;
  #aborted = false;

  get aborted() { return this.#aborted; }
  get reason() { return this.#reason; }
  onabort = null;

  throwIfAborted() {
    if (this.#aborted) throw this.#reason;
  }

  addEventListener(type, callback, options) {
    if (type === 'abort' && callback) this.#listeners.set(callback, options?.once);
  }

  removeEventListener(type, callback) {
    if (type === 'abort') this.#listeners.delete(callback);
  }

  static abort(reason) {
    const controller = new AbortController();
    controller.abort(reason);
    return controller.signal;
  }

  static any(signals) {
    const controller = new AbortController();
    for (const signal of signals) {
      if (signal.aborted) return AbortSignal.abort(signal.reason);
      signal.addEventListener('abort', () => controller.abort(signal.reason), { once: true });
    }
    return controller.signal;
  }

  static controller() {
    const signal = new AbortSignal();
    return {
      signal,
      abort(reason = new DOMException('Request aborted', 'AbortError')) {
        if (signal.#aborted) return;
        signal.#reason = reason;
        signal.#aborted = true;
        const event = { type: 'abort', target: signal, currentTarget: signal };
        signal.onabort?.(event);
        for (const [callback, once] of signal.#listeners) {
          if (once) signal.#listeners.delete(callback);
          if (typeof callback === 'function') callback.call(signal, event);
          else callback.handleEvent(event);
        }
      },
    };
  }
}

export class AbortController {
  #controller = AbortSignal.controller();

  get signal() { return this.#controller.signal; }
  abort(reason) { this.#controller.abort(reason); }
}
