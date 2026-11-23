import { dispatch } from './dispatch.js';
import { URL, URLSearchParams } from 'whatwg-url-without-unicode';
import { ReadableStream, TransformStream, WritableStream } from 'web-streams-polyfill';
import { AbortController, AbortSignal, DOMException } from './abort.js';
import { TextDecoder, TextEncoder } from './text.js';
import { Blob, File, FormData, Headers, Request, Response } from './web.js';

Object.assign(globalThis, {
  AbortController, AbortSignal, Blob, DOMException, File, FormData, Headers, ReadableStream,
  Request, Response, TextDecoder, TextEncoder, TransformStream, URL, URLSearchParams, WritableStream,
});

const { op_worker_call: call, op_worker_body_read: read, op_worker_failure: failure } = globalThis;
delete globalThis.op_worker_call;
delete globalThis.op_worker_body_read;
delete globalThis.op_worker_failure;

export const { invoke, shutdown } = dispatch(call, read, failure);
