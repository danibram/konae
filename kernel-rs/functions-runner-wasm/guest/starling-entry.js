import { hostRuntime, initializationFailed } from './starling-runtime.js';
import * as web from './web.js';
import * as app from 'konae:app-source';

if (initializationFailed()) {
  throw new Error(`function.wasm-init-effect: A function cannot start timers or fetch during top-level initialization.`);
}

Object.assign(globalThis, {
  AbortController: web.AbortController,
  AbortSignal: web.AbortSignal,
  Blob: web.Blob,
  DOMException: web.DOMException,
  File: web.File,
  FormData: web.FormData,
  Headers: web.Headers,
  ReadableStream: web.ReadableStream,
  Request: web.Request,
  Response: web.Response,
});

export const { handle, hasShutdown, shutdown, spin } = hostRuntime(app);
