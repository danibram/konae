import { hostRuntime } from './starling-runtime.js';
import * as app from 'konae:app-source';

export const { handle, hasShutdown, shutdown, spin } = hostRuntime(app);
