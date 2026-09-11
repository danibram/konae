import kernelAuthSource from "../../kernel-rs/src/auth.rs?raw";
import kernelCargoLock from "../../kernel-rs/Cargo.lock?raw";
import kernelCargoManifest from "../../kernel-rs/Cargo.toml?raw";
import kernelMainSource from "../../kernel-rs/src/main.rs?raw";
import type { ExportFile } from "./export-project-kit";

export const kernelFiles: ExportFile[] = [
  { path: "kernel-rs/Cargo.toml", contents: kernelCargoManifest },
  { path: "kernel-rs/Cargo.lock", contents: kernelCargoLock },
  { path: "kernel-rs/src/main.rs", contents: kernelMainSource },
  { path: "kernel-rs/src/auth.rs", contents: kernelAuthSource },
];
