import kernelOauthSource from "../../kernel-rs/src/oauth.rs?raw";
import kernelMcpSource from "../../kernel-rs/src/mcp.rs?raw";
import kernelOwnerPage from "../../kernel-rs/src/owner-login.html?raw";
import kernelAuthSource from "../../kernel-rs/src/auth.rs?raw";
import kernelAccountsSource from "../../kernel-rs/src/accounts.rs?raw";
import kernelAdmissionSource from "../../kernel-rs/src/admission.rs?raw";
import kernelInvitationsSource from "../../kernel-rs/src/invitations.rs?raw";
import kernelClerkSource from "../../kernel-rs/src/clerk.rs?raw";
import kernelCopiesSource from "../../kernel-rs/src/copies.rs?raw";
import kernelEjectSource from "../../kernel-rs/src/eject.rs?raw";
import kernelLimitsSource from "../../kernel-rs/src/limits.rs?raw";
import kernelHttpSource from "../../kernel-rs/src/http.rs?raw";
import kernelHttpBindingCases from "../../kernel-rs/src/http-binding-cases.json?raw";
import kernelSignatureCases from "../../kernel-rs/src/signature-cases.json?raw";
import kernelMailSource from "../../kernel-rs/src/mail.rs?raw";
import kernelMigrationSource from "../../kernel-rs/src/migration.rs?raw";
import kernelPackageSource from "../../kernel-rs/src/package.rs?raw";
import kernelRecoverySource from "../../kernel-rs/src/recovery.rs?raw";
import kernelSecuritySource from "../../kernel-rs/src/security.rs?raw";
import kernelSqlRulesSource from "../../kernel-rs/src/sql_rules.rs?raw";
import kernelSqlRuleCases from "../../kernel-rs/src/sql-rule-cases.json?raw";
import kernelUsageSource from "../../kernel-rs/src/usage.rs?raw";
import kernelSignalCases from "../../kernel-rs/src/signal-cases.json?raw";
import kernelCargoLock from "../../kernel-rs/Cargo.lock?raw";
import kernelCargoManifest from "../../kernel-rs/Cargo.toml?raw";
import kernelMainSource from "../../kernel-rs/src/main.rs?raw";
import kernelSdkGuide from "../../kernel-rs/src/sdk-guide.json?raw";
import type { ExportFile } from "./export-project-kit";

export const kernelFiles: ExportFile[] = [
  { path: "kernel-rs/src/oauth.rs", contents: kernelOauthSource },
  { path: "kernel-rs/src/mcp.rs", contents: kernelMcpSource },
  { path: "kernel-rs/src/owner-login.html", contents: kernelOwnerPage },
  { path: "kernel-rs/Cargo.toml", contents: kernelCargoManifest },
  { path: "kernel-rs/Cargo.lock", contents: kernelCargoLock },
  { path: "kernel-rs/src/main.rs", contents: kernelMainSource },
  { path: "kernel-rs/src/auth.rs", contents: kernelAuthSource },
  { path: "kernel-rs/src/accounts.rs", contents: kernelAccountsSource },
  { path: "kernel-rs/src/admission.rs", contents: kernelAdmissionSource },
  { path: "kernel-rs/src/invitations.rs", contents: kernelInvitationsSource },
  { path: "kernel-rs/src/clerk.rs", contents: kernelClerkSource },
  { path: "kernel-rs/src/copies.rs", contents: kernelCopiesSource },
  { path: "kernel-rs/src/eject.rs", contents: kernelEjectSource },
  { path: "kernel-rs/src/limits.rs", contents: kernelLimitsSource },
  { path: "kernel-rs/src/http.rs", contents: kernelHttpSource },
  { path: "kernel-rs/src/http-binding-cases.json", contents: kernelHttpBindingCases },
  { path: "kernel-rs/src/signature-cases.json", contents: kernelSignatureCases },
  { path: "kernel-rs/src/mail.rs", contents: kernelMailSource },
  { path: "kernel-rs/src/migration.rs", contents: kernelMigrationSource },
  { path: "kernel-rs/src/package.rs", contents: kernelPackageSource },
  { path: "kernel-rs/src/recovery.rs", contents: kernelRecoverySource },
  { path: "kernel-rs/src/sdk-guide.json", contents: kernelSdkGuide },
  { path: "kernel-rs/src/security.rs", contents: kernelSecuritySource },
  { path: "kernel-rs/src/sql_rules.rs", contents: kernelSqlRulesSource },
  { path: "kernel-rs/src/sql-rule-cases.json", contents: kernelSqlRuleCases },
  { path: "kernel-rs/src/usage.rs", contents: kernelUsageSource },
  { path: "kernel-rs/src/signal-cases.json", contents: kernelSignalCases },
];
