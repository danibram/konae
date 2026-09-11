import kernelOauthSource from "../../kernel-rs/src/oauth.rs?raw";
import kernelMcpSource from "../../kernel-rs/src/mcp.rs?raw";
import kernelOwnerPage from "../../kernel-rs/src/owner-login.html?raw";
import kernelAuthSource from "../../kernel-rs/src/auth.rs?raw";
import kernelClerkSource from "../../kernel-rs/src/clerk.rs?raw";
import kernelLimitsSource from "../../kernel-rs/src/limits.rs?raw";
import kernelCargoLock from "../../kernel-rs/Cargo.lock?raw";
import kernelCargoManifest from "../../kernel-rs/Cargo.toml?raw";
import kernelMainSource from "../../kernel-rs/src/main.rs?raw";
import type { ExportFile } from "./export-project-kit";

export const kernelFiles: ExportFile[] = [
  { path: "kernel-rs/src/oauth.rs", contents: kernelOauthSource },
  { path: "kernel-rs/src/mcp.rs", contents: kernelMcpSource },
  { path: "kernel-rs/src/owner-login.html", contents: kernelOwnerPage },
  { path: "kernel-rs/Cargo.toml", contents: kernelCargoManifest },
  { path: "kernel-rs/Cargo.lock", contents: kernelCargoLock },
  { path: "kernel-rs/src/main.rs", contents: kernelMainSource },
  { path: "kernel-rs/src/auth.rs", contents: kernelAuthSource },
  { path: "kernel-rs/src/clerk.rs", contents: kernelClerkSource },
  { path: "kernel-rs/src/limits.rs", contents: kernelLimitsSource },
];
