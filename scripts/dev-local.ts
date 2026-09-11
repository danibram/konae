import { spawn, type ChildProcess } from "node:child_process";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const kernelPort = process.env.LITTLEBOX_KERNEL_PORT ?? "8790";
const editorPort = process.env.LITTLEBOX_EDITOR_PORT ?? "4173";
const kernelUrl = process.env.LITTLEBOX_KERNEL_URL ?? `http://127.0.0.1:${kernelPort}`;
const dataRootSetting = process.env.LITTLEBOX_DATA_DIR ?? "data/kernel-local";
const dataRoot = path.isAbsolute(dataRootSetting) ? dataRootSetting : path.resolve(root, dataRootSetting);

await mkdir(dataRoot, { recursive: true });

const children: ChildProcess[] = [];
const finishedChildren = new Set<ChildProcess>();
let stopping = false;
let resolveRun: ((code: number) => void) | undefined;
const runFinished = new Promise<number>((resolve) => { resolveRun = resolve; });

function start(command: string, args: string[], env: NodeJS.ProcessEnv) {
  const child = spawn(command, args, { cwd: root, env, stdio: "inherit" });
  children.push(child);
  child.on("error", (error) => {
    finishedChildren.add(child);
    console.error(`Could not start ${command}: ${error.message}`);
    stop(1);
  });
  child.on("exit", (code, signal) => {
    finishedChildren.add(child);
    if (stopping) return;
    const exitCode = code ?? (signal ? 1 : 0);
    if (exitCode !== 0) console.error(`${command} stopped with exit code ${exitCode}.`);
    stop(exitCode);
  });
  return child;
}

function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  resolveRun?.(code);
  for (const child of children) child.kill("SIGTERM");
  const forceStop = setTimeout(() => {
    for (const child of children) child.kill("SIGKILL");
  }, 5_000);
  forceStop.unref();
}

process.on("SIGINT", () => stop(0));
process.on("SIGTERM", () => stop(0));

const kernelEnv = {
  LITTLEBOX_BUN_PATH: process.execPath,
  LITTLEBOX_SOURCE_ROOT: root,
  ...process.env,
  LITTLEBOX_KERNEL_PORT: kernelPort,
  LITTLEBOX_DATA_DIR: dataRoot,
  LITTLEBOX_PUBLIC_ORIGIN: process.env.LITTLEBOX_PUBLIC_ORIGIN ?? `http://localhost:${kernelPort}`,
  LITTLEBOX_HOST_SUFFIX: process.env.LITTLEBOX_HOST_SUFFIX ?? "localhost",
};
delete kernelEnv.LITTLEBOX_S3_ENDPOINT;
delete kernelEnv.LITTLEBOX_S3_ACCESS_KEY;
delete kernelEnv.LITTLEBOX_S3_SECRET_KEY;
delete kernelEnv.LITTLEBOX_S3_REGION;
delete kernelEnv.LITTLEBOX_ARTIFACT_BUCKET;
delete kernelEnv.LITTLEBOX_DATA_BUCKET;
const editorEnv = {
  ...process.env,
  LITTLEBOX_KERNEL_URL: kernelUrl,
  LITTLEBOX_BUN_PATH: process.execPath,
};

console.log(`Littlebox editor: http://localhost:${editorPort}`);
console.log(`Littlebox Kernel: ${kernelUrl}`);
console.log(`Local app data: ${dataRoot}`);

start("cargo", ["run", "--manifest-path", path.join(root, "kernel-rs", "Cargo.toml")], kernelEnv);
start(process.execPath, ["run", "dev", "--", "--host", "127.0.0.1", "--port", editorPort, "--strictPort"], editorEnv);

const exitCode = await runFinished;
await Promise.all(children.map((child) => new Promise<void>((resolve) => {
  if (finishedChildren.has(child) || child.exitCode !== null || child.signalCode !== null) return resolve();
  child.once("exit", () => resolve());
  child.once("error", () => resolve());
})));
process.exitCode = exitCode;
