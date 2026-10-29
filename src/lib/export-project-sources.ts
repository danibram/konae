import kernelOauthSource from "../../kernel-rs/src/oauth.rs?raw";
import kernelMcpSource from "../../kernel-rs/src/mcp.rs?raw";
import kernelOwnerPage from "../../kernel-rs/src/owner-login.html?raw";
import kernelAuthSource from "../../kernel-rs/src/auth.rs?raw";
import kernelAccountsSource from "../../kernel-rs/src/accounts.rs?raw";
import kernelActionShapeSource from "../../kernel-rs/src/action_shape.rs?raw";
import kernelActionCases from "../../kernel-rs/src/action-cases.json?raw";
import kernelAdmissionSource from "../../kernel-rs/src/admission.rs?raw";
import kernelAppOwnerSource from "../../kernel-rs/src/app_owner.rs?raw";
import kernelArtifactSource from "../../kernel-rs/src/artifact.rs?raw";
import kernelInvitationsSource from "../../kernel-rs/src/invitations.rs?raw";
import kernelClerkSource from "../../kernel-rs/src/clerk.rs?raw";
import kernelGoogleSource from "../../kernel-rs/src/google.rs?raw";
import kernelCopiesSource from "../../kernel-rs/src/copies.rs?raw";
import kernelDomainSource from "../../kernel-rs/src/domain.rs?raw";
import kernelDomainCases from "../../kernel-rs/src/domain-cases.json?raw";
import kernelGoogleClientCases from "../../kernel-rs/src/google-client-cases.json?raw";
import kernelEjectSource from "../../kernel-rs/src/eject.rs?raw";
import kernelFileStoreSource from "../../kernel-rs/src/file_store.rs?raw";
import kernelAppDbSource from "../../kernel-rs/src/app_db.rs?raw";
import kernelHostsSource from "../../kernel-rs/src/hosts.rs?raw";
import kernelSqliteBenchSource from "../../kernel-rs/src/sqlite_bench.rs?raw";
import kernelFilesSource from "../../kernel-rs/src/files.rs?raw";
import kernelGuardSource from "../../kernel-rs/src/guard.rs?raw";
import kernelCallsSource from "../../kernel-rs/src/calls.rs?raw";
import kernelChecksSource from "../../kernel-rs/src/checks.rs?raw";
import kernelCheckCarryCases from "../../kernel-rs/src/check-carry-cases.json?raw";
import kernelCallCases from "../../kernel-rs/src/call-cases.json?raw";
import kernelGuardCases from "../../kernel-rs/src/guard-cases.json?raw";
import kernelScheduleSource from "../../kernel-rs/src/schedule.rs?raw";
import kernelScheduleCases from "../../kernel-rs/src/schedule-cases.json?raw";
import kernelGuideSource from "../../kernel-rs/src/guide.rs?raw";
import kernelLimitsSource from "../../kernel-rs/src/limits.rs?raw";
import kernelHttpSource from "../../kernel-rs/src/http.rs?raw";
import kernelHttpBindingCases from "../../kernel-rs/src/http-binding-cases.json?raw";
import kernelSignatureCases from "../../kernel-rs/src/signature-cases.json?raw";
import kernelMailSource from "../../kernel-rs/src/mail.rs?raw";
import kernelMigrationSource from "../../kernel-rs/src/migration.rs?raw";
import kernelModelRulesSource from "../../kernel-rs/src/model_rules.rs?raw";
import kernelNotifySource from "../../kernel-rs/src/notify.rs?raw";
import kernelNotifyCases from "../../kernel-rs/src/notify-cases.json?raw";
import kernelNotifyColumnCases from "../../kernel-rs/src/notify-column-cases.json?raw";
import kernelPackageSource from "../../kernel-rs/src/package.rs?raw";
import kernelPathSource from "../../kernel-rs/src/path.rs?raw";
import kernelPathStateCases from "../../kernel-rs/src/path-state-cases.json?raw";
import kernelPathNameSource from "../../kernel-rs/src/path_name.rs?raw";
import kernelPathCases from "../../kernel-rs/src/path-cases.json?raw";
import kernelProviderSource from "../../kernel-rs/src/provider.rs?raw";
import kernelProviderSignatureCases from "../../kernel-rs/src/provider-signature-cases.json?raw";
import kernelSignaturePresets from "../../kernel-rs/src/signature-presets.json?raw";
import kernelSignaturePresetCases from "../../kernel-rs/src/signature-preset-cases.json?raw";
import kernelRefusalSource from "../../kernel-rs/src/refusal.rs?raw";
import kernelRefusalCodes from "../../kernel-rs/src/refusal-codes.json?raw";
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
  { path: "kernel-rs/src/action_shape.rs", contents: kernelActionShapeSource },
  { path: "kernel-rs/src/action-cases.json", contents: kernelActionCases },
  { path: "kernel-rs/src/admission.rs", contents: kernelAdmissionSource },
  { path: "kernel-rs/src/app_owner.rs", contents: kernelAppOwnerSource },
  { path: "kernel-rs/src/artifact.rs", contents: kernelArtifactSource },
  { path: "kernel-rs/src/invitations.rs", contents: kernelInvitationsSource },
  { path: "kernel-rs/src/clerk.rs", contents: kernelClerkSource },
  { path: "kernel-rs/src/google.rs", contents: kernelGoogleSource },
  { path: "kernel-rs/src/copies.rs", contents: kernelCopiesSource },
  { path: "kernel-rs/src/domain.rs", contents: kernelDomainSource },
  { path: "kernel-rs/src/domain-cases.json", contents: kernelDomainCases },
  { path: "kernel-rs/src/google-client-cases.json", contents: kernelGoogleClientCases },
  { path: "kernel-rs/src/eject.rs", contents: kernelEjectSource },
  { path: "kernel-rs/src/file_store.rs", contents: kernelFileStoreSource },
  { path: "kernel-rs/src/app_db.rs", contents: kernelAppDbSource },
  { path: "kernel-rs/src/hosts.rs", contents: kernelHostsSource },
  { path: "kernel-rs/src/sqlite_bench.rs", contents: kernelSqliteBenchSource },
  { path: "kernel-rs/src/files.rs", contents: kernelFilesSource },
  { path: "kernel-rs/src/guard.rs", contents: kernelGuardSource },
  { path: "kernel-rs/src/calls.rs", contents: kernelCallsSource },
  { path: "kernel-rs/src/checks.rs", contents: kernelChecksSource },
  { path: "kernel-rs/src/check-carry-cases.json", contents: kernelCheckCarryCases },
  { path: "kernel-rs/src/call-cases.json", contents: kernelCallCases },
  { path: "kernel-rs/src/guide.rs", contents: kernelGuideSource },
  { path: "kernel-rs/src/guard-cases.json", contents: kernelGuardCases },
  { path: "kernel-rs/src/schedule.rs", contents: kernelScheduleSource },
  { path: "kernel-rs/src/schedule-cases.json", contents: kernelScheduleCases },
  { path: "kernel-rs/src/limits.rs", contents: kernelLimitsSource },
  { path: "kernel-rs/src/http.rs", contents: kernelHttpSource },
  { path: "kernel-rs/src/http-binding-cases.json", contents: kernelHttpBindingCases },
  { path: "kernel-rs/src/signature-cases.json", contents: kernelSignatureCases },
  { path: "kernel-rs/src/mail.rs", contents: kernelMailSource },
  { path: "kernel-rs/src/notify.rs", contents: kernelNotifySource },
  { path: "kernel-rs/src/notify-cases.json", contents: kernelNotifyCases },
  { path: "kernel-rs/src/notify-column-cases.json", contents: kernelNotifyColumnCases },
  { path: "kernel-rs/src/migration.rs", contents: kernelMigrationSource },
  { path: "kernel-rs/src/model_rules.rs", contents: kernelModelRulesSource },
  { path: "kernel-rs/src/package.rs", contents: kernelPackageSource },
  { path: "kernel-rs/src/path.rs", contents: kernelPathSource },
  { path: "kernel-rs/src/path-state-cases.json", contents: kernelPathStateCases },
  { path: "kernel-rs/src/path_name.rs", contents: kernelPathNameSource },
  { path: "kernel-rs/src/path-cases.json", contents: kernelPathCases },
  { path: "kernel-rs/src/provider.rs", contents: kernelProviderSource },
  { path: "kernel-rs/src/provider-signature-cases.json", contents: kernelProviderSignatureCases },
  { path: "kernel-rs/src/signature-presets.json", contents: kernelSignaturePresets },
  { path: "kernel-rs/src/signature-preset-cases.json", contents: kernelSignaturePresetCases },
  { path: "kernel-rs/src/recovery.rs", contents: kernelRecoverySource },
  { path: "kernel-rs/src/refusal.rs", contents: kernelRefusalSource },
  { path: "kernel-rs/src/refusal-codes.json", contents: kernelRefusalCodes },
  { path: "kernel-rs/src/sdk-guide.json", contents: kernelSdkGuide },
  { path: "kernel-rs/src/security.rs", contents: kernelSecuritySource },
  { path: "kernel-rs/src/sql_rules.rs", contents: kernelSqlRulesSource },
  { path: "kernel-rs/src/sql-rule-cases.json", contents: kernelSqlRuleCases },
  { path: "kernel-rs/src/usage.rs", contents: kernelUsageSource },
  { path: "kernel-rs/src/signal-cases.json", contents: kernelSignalCases },
];
