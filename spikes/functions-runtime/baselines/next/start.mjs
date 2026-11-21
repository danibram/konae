import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const child = spawn(process.execPath, [
  fileURLToPath(import.meta.resolve('next/dist/bin/next')),
  'start', fileURLToPath(new URL('.', import.meta.url)),
  '--hostname', '127.0.0.1', '--port', process.env.PORT ?? '43104',
], { stdio: 'inherit', env: { ...process.env, NODE_ENV: 'production' } });
process.once('SIGTERM', () => child.kill('SIGTERM'));
process.once('SIGINT', () => child.kill('SIGINT'));
child.on('exit', (code) => process.exit(code ?? 1));
