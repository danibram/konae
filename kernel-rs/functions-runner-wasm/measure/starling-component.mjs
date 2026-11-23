import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const [sourcePath, outputPath, expectedEngine] = process.argv.slice(2);
const guest = fileURLToPath(new URL('../guest/', import.meta.url));
const requireTool = createRequire(resolve(process.env.KONAE_STARLING_TOOLS, 'package.json'));
const packageRoot = resolve(process.env.KONAE_STARLING_TOOLS, 'node_modules/@bytecodealliance/componentize-js');
const componentizePath = resolve(packageRoot, 'src/componentize.js');
const pkg = JSON.parse(await readFile(resolve(packageRoot, 'package.json'), 'utf8'));
const engine = resolve(packageRoot, 'lib/starlingmonkey_embedding.wasm');
const digest = createHash('sha256').update(await readFile(engine)).digest('hex');
if (`${pkg.version}:${digest}` !== expectedEngine) throw Error('Unexpected ComponentizeJS or StarlingMonkey version');
const { init, parse } = requireTool('es-module-lexer');
const { build } = requireTool('esbuild');
const { componentize } = await import(pathToFileURL(componentizePath));

await init;
const source = await readFile(sourcePath, 'utf8');
const [imports] = parse(source);
if (imports.some(entry => entry.d === -1)) {
  console.error('The bundle has a static import');
  process.exit(65);
}

let app = source;
for (const entry of imports.toReversed()) {
  if (entry.d >= 0) {
    // Keep evaluation of the specifier expression and reject the dynamic import.
    app = app.slice(0, entry.ss)
      + '((..._) => Promise.reject(new TypeError("function_import_forbidden")))'
      + app.slice(entry.ss + 'import'.length);
  }
}

const [bundled] = (await build({
  entryPoints: [resolve(guest, 'starling-entry.js')], bundle: true, format: 'esm',
  platform: 'neutral', write: false, external: ['konae:worker/host'],
  plugins: [{
    name: 'app-source',
    setup(builder) {
      builder.onResolve({ filter: /^konae:app-source$/ }, () => ({ path: 'app.mjs', namespace: 'app' }));
      builder.onLoad({ filter: /.*/, namespace: 'app' }, () => ({ contents: app, loader: 'js' }));
    },
  }],
})).outputFiles;
const witWorld = await readFile(resolve(guest, 'starling.wit'), 'utf8');
const { component } = await componentize(bundled.text, {
  engine, witWorld, disableFeatures: ['stdio', 'random', 'clocks', 'http', 'fetch-event'],
});
await writeFile(outputPath, component);
