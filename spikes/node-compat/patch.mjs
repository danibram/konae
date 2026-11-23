function replace(source, before, after) {
  if (source.split(before).length !== 2) throw Error(`Builder seam changed: ${before}`);
  return source.replace(before, after);
}

export function patchLocal(source, providerModule) {
  source = `import { globals, nodeCompat } from '../../spikes/node-compat/policy.mjs';
    import { npmMode } from '../../spikes/node-compat/npm-mode.mjs';
    import { providers } from ${JSON.stringify(providerModule)};\n` + source;
  source = replace(source,
    'minify: !development, logOverride:',
    'minify: !development, define: { process: "process", global: "globalThis" }, logOverride:',
  );
  return replace(source,
    'plugins: [{ name: "declared-server-packages", setup(build) {',
    'inject: [globals], plugins: [nodeCompat((name) => providers.get(name)), npmMode, { name: "declared-server-packages", setup(build) {',
  );
}

export function patchEditor(source) {
  source = `import { globals, nodeCompat } from '../../spikes/node-compat/policy.mjs';\n` + source;
  source = replace(source,
    'define: { "process.env.NODE_ENV": JSON.stringify(release ? "production" : "development") },',
    '...(server ? { inject: [globals], define: { process: "process", global: "globalThis" } } : { define: { "process.env.NODE_ENV": JSON.stringify(release ? "production" : "development") } }),',
  );
  return replace(source,
    'plugins: [projectFilesPlugin(files, project, server), packagesPlugin(declaredPackages(files))],',
    'plugins: [...(server ? [nodeCompat(async (name) => (await fetch("/__node_compat/" + name)).text())] : []), projectFilesPlugin(files, project, server), packagesPlugin(declaredPackages(files))],',
  );
}
