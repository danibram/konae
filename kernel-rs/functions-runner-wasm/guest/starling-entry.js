import { hostRuntime, initializationFailed } from './starling-runtime.js';
import * as web from './web.js';
import * as app from 'konae:app-source';

if (initializationFailed()) {
  throw new Error(`function.initialization-cannot-snapshot: The function's top-level code must finish without pending promises, timers, fetch, or random values.`);
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
