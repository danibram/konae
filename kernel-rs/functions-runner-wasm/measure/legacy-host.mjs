import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { serve } from '../../../spikes/functions-runtime/baselines/node_modules/@hono/node-server/dist/index.mjs';
import { createHost } from '../../../spikes/functions-runtime/baselines/lib/host.mjs';

const limits = { heap_bytes: 67108864, response_bytes: 2097152, rss_bytes: 268435456, operation_limit: 64 };

function bundle(name) {
  const source = readFileSync(new URL(`../../../spikes/functions-runtime/app/dist/${name}.js`, import.meta.url), 'utf8');
  // The old suite used context.data. Wrap its unchanged bundle at the IPC boundary.
  const adapter = `
    export async function wasmHandle(request, context) {
      return handle(request, {actor: context.actor, data: {
        readOrder: (id) => context.actions.call('readOrder', {id}),
        updateOrder: (id, quantity) => context.actions.call('updateOrder', {id, quantity}),
      }});
    }
  `;
  const exports = /export\s*{\s*handle\s*};\s*$/;
  if (!exports.test(source)) throw Error('Expected the original handle export');
  return source.replace(exports, '') + adapter + '\nexport { wasmHandle as handle };';
}

class Worker {
  constructor(name) {
    this.child = spawn(process.env.KONAE_FUNCTIONS_RUNNER, ['--function-worker'], {
      stdio: ['pipe', 'pipe', 'inherit'],
    });
    this.iterator = this.child.stdout[Symbol.asyncIterator]();
    this.buffer = Buffer.alloc(0);
    this.generation = randomUUID();
    this.number = 0;
    this.tail = Promise.resolve();
    this.send({
      kind: 'Init', abi: 'konae-functions-ipc/2', generation: this.generation,
      function: name, source: bundle(name),
      limits,
    });
  }

  send(value) {
    const json = Buffer.from(JSON.stringify(value));
    const length = Buffer.alloc(4);
    length.writeUInt32BE(json.length);
    this.child.stdin.write(Buffer.concat([length, json]));
  }

  async frame() {
    while (this.buffer.length < 4 || this.buffer.length < 4 + this.buffer.readUInt32BE()) {
      const { value, done } = await this.iterator.next();
      if (done) throw Error('Worker exited before its reply');
      this.buffer = Buffer.concat([this.buffer, value]);
    }
    const end = 4 + this.buffer.readUInt32BE();
    const result = JSON.parse(this.buffer.subarray(4, end));
    this.buffer = this.buffer.subarray(end);
    return result;
  }

  async ready() {
    const message = await this.frame();
    if (message.kind !== 'Ready' || message.generation !== this.generation) throw Error('Expected Ready');
  }

  invoke(request, context) {
    const result = this.tail.then(() => this.invokeSequential(request, context));
    this.tail = result.catch(() => {});
    return result;
  }

  async invokeSequential(request, context) {
    const body = Buffer.from(await request.arrayBuffer());
    let offset = 0;
    const capability = { generation: this.generation, request: ++this.number, token: randomUUID() };
    this.send({
      kind: 'Invoke', capability,
      input: { url: request.url, method: request.method, headers: [...request.headers],
        body_len: body.length, actor: context.actor.userId },
    });
    for (;;) {
      const message = await this.frame();
      if (message.capability?.token !== capability.token || message.capability.request !== capability.request ||
          message.capability.generation !== capability.generation) throw Error('Unexpected capability');
      const sequence = message.sequence;
      if (message.kind === 'BodyRead') {
        const bytes = body.subarray(offset, offset + message.maximum);
        offset += bytes.length;
        this.send({ kind: 'BodyChunk', capability, sequence, body_len: bytes.length, bytes: [...bytes], done: !bytes.length });
      } else if (message.kind === 'Operation') {
        let result;
        try {
          const { name, input } = message.args;
          if (message.operation !== 'action') throw Error('Unknown operation');
          const value = name === 'readOrder' ? context.data.readOrder(input.id)
            : name === 'updateOrder' ? context.data.updateOrder(input.id, input.quantity) : undefined;
          if (value === undefined) throw Error('Unknown action');
          result = { Ok: value };
        } catch {
          result = { Err: 'internal_error' };
        }
        this.send({ kind: 'OperationResult', capability, sequence, result });
      } else if (message.kind === 'Reply') {
        const { body, headers, status } = message.output;
        return new Response(body, { headers, status });
      } else if (message.kind === 'Failure') {
        throw Error(message.code);
      } else throw Error('Unexpected worker message');
    }
  }
}

if (process.argv[2] === '--write-bundles') {
  for (const name of ['orders', 'preferences']) {
    writeFileSync(join(process.argv[3], name + '.mjs'), bundle(name));
  }
} else {
  const orders = new Worker('orders');
  const preferences = new Worker('preferences');
  await Promise.all([orders.ready(), preferences.ready()]);
  const host = createHost({
    directory: process.env.DATA_DIR,
    orders: (request, context) => orders.invoke(request, context),
    preferences: (request, context) => preferences.invoke(request, context),
  });
  serve({ hostname: '127.0.0.1', port: Number(process.env.PORT), fetch: host.fetch });
}
