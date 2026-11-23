import { hostRuntime } from './starling-runtime.js';
import * as web from './web.js';
import * as app from 'konae:app-source';

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
});

export const { handle, hasShutdown, shutdown, spin } = hostRuntime(app);
