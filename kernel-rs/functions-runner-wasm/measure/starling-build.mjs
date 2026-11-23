import { componentize } from '@bytecodealliance/componentize-js';
import { readFile, writeFile } from 'node:fs/promises';

const [ordersPath, output] = process.argv.slice(2);
if (!ordersPath || !output) throw Error('Expected the Hono orders ESM path and an output path');
const orders = await readFile(ordersPath, 'utf8');
const exportList = /export\s*{\s*handle\s*};\s*$/;
if (!exportList.test(orders)) throw Error('Expected the original handle export');
const source = orders.replace(exportList, '') + `
import { readOrder } from 'konae:starling/host';

async function componentHandle(url) {
  const response = await handle(new Request(url), {
    actor: { userId: 'alice' },
    data: { readOrder: id => JSON.parse(readOrder(id)) },
  });
  return JSON.stringify({ status: response.status, body: await response.json() });
}
export { componentHandle as handle };
`;
const witWorld = `
  package konae:starling;
  interface host { read-order: func(id: u32) -> string; }
  world runner {
    import host;
    export handle: func(url: string) -> string;
  }
`;
const started = performance.now();
const { component, imports } = await componentize(source, {
  witWorld, disableFeatures: ['stdio', 'random', 'clocks', 'http', 'fetch-event'],
});
await writeFile(output, component);
console.log(JSON.stringify({ ms: performance.now() - started, bytes: component.byteLength, imports }));
