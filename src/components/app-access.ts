import type { ApiKey, AppInvitation, AppUserState, InvitationDelivery, InvitationResult, MailStatus, SignupMode } from "../deployments/client";
import type { DeploymentVisibility } from "../deployments/manifest";
import { parseServerActionSource, type ServerActionDefinition } from "../kernel/actions";

/**
 * What the Users and Integrations views and the publish dialog say. Kept apart
 * from the components so the words and the state rules have tests. Every
 * sentence says "app account" for a person who uses the app and "owner" for
 * the person who builds it, so creating an app never reads like signing in to one.
 */

export type Tone = "neutral" | "accent" | "success" | "warning" | "danger";

/** One sentence per mode. Allowlist is the advanced option the Kernel keeps for older apps. */
export const signupModeCopy: Record<SignupMode, { label: string; sentence: string; advanced?: true }> = {
  open: { label: "Open", sentence: "Anyone who opens the published app can create an app account." },
  invitation: { label: "Invitation only", sentence: "Only people you invite by email can create an app account, each with a link that works once within seven days." },
  closed: { label: "Closed", sentence: "No one can sign up, people you invite can still accept, and existing app accounts still sign in." },
  allowlist: { label: "Allowlist", sentence: "Only the emails and @domains you list can create an app account, and listing an address sends no invitation.", advanced: true },
};

/** The modes a first publish offers. Allowlist needs its list, so it waits for the Users view. */
export const firstPublishModes: SignupMode[] = ["invitation", "open", "closed"];

/** A private app proposes Invitation only; a public one proposes Open. */
export function defaultSignupMode(visibility: DeploymentVisibility): SignupMode {
  return visibility === "private" ? "invitation" : "open";
}

export const visibilityCopy: Record<DeploymentVisibility, { label: string; sentence: string }> = {
  public: { label: "Public", sentence: "Anyone with the link opens the app; actions marked Anyone run without an app account." },
  private: { label: "Private", sentence: "A person needs an active app account before the app loads." },
};

/**
 * What a republish says about access. The Kernel keeps the current signup
 * policy on a republish, except that publishing a public app as private turns
 * Open into Closed (docs/mcp.md, "Signup policy").
 */
export function republishAccessNote(current: { visibility: DeploymentVisibility; mode?: SignupMode }, next: DeploymentVisibility): string {
  if (current.visibility === "public" && next === "private" && current.mode === "open") {
    return "This publish makes the app private, so the Kernel turns Open signup into Closed. Invite people or change the policy in Auth > Users afterwards.";
  }
  if (current.visibility !== next) {
    return `This publish makes the app ${next}. Who can create an app account stays as it is; change it in Auth > Users.`;
  }
  return "Publishing again keeps who can open the app and who can create an app account. Change them in Auth > Users.";
}

export const MAX_INVITATION_BATCH = 50;

/** Splits on new lines, commas and spaces, trims, lowercases and drops repeats. */
export function parseEmailBatch(text: string): { emails: string[]; tooMany: boolean } {
  const seen = new Set<string>();
  for (const entry of text.split(/[\s,;]+/)) {
    const email = entry.trim().toLowerCase();
    if (email) seen.add(email);
  }
  const emails = [...seen];
  return { emails, tooMany: emails.length > MAX_INVITATION_BATCH };
}

/**
 * The idempotency key for a batch. A retry of the same addresses after a lost
 * answer reuses the key, so the Kernel returns the first answer and sends
 * nothing twice. A different list gets a new key.
 */
export type PendingBatch = { emails: string; key: string };

export function batchKey(previous: PendingBatch | null, emails: string[], newKey: () => string = () => `invite-${crypto.randomUUID()}`): PendingBatch {
  const signature = [...emails].sort().join("\n");
  if (previous && previous.emails === signature) return previous;
  return { emails: signature, key: newKey() };
}

export const userStateView: Record<AppUserState, { label: string; tone: Tone; sentence: string }> = {
  active: { label: "Active", tone: "success", sentence: "Signs in to the app." },
  pending: { label: "Pending", tone: "warning", sentence: "Has not verified the email or accepted the invitation yet." },
  suspended: { label: "Suspended", tone: "danger", sentence: "Cannot sign in. The app account and its rows stay." },
};

const statusView: Record<AppInvitation["status"], { label: string; tone: Tone }> = {
  pending: { label: "Pending", tone: "accent" },
  expired: { label: "Expired", tone: "warning" },
  "needs-resend": { label: "Needs resend", tone: "warning" },
  "already-member": { label: "Has an app account", tone: "success" },
  accepted: { label: "Accepted", tone: "success" },
  revoked: { label: "Revoked", tone: "neutral" },
};

/**
 * The Kernel records, per invitation, whether the mail transport took the
 * message. It does not record whether the provider accepted or delivered it;
 * those counts exist only for the whole queue in `mail_status`.
 */
export const deliveryView: Record<InvitationDelivery, { label: string; tone: Tone }> = {
  queued: { label: "Handed to mail", tone: "neutral" },
  failed: { label: "Sending failed", tone: "danger" },
  "not-sent": { label: "No email sent", tone: "warning" },
};

export function invitationView(invitation: Pick<AppInvitation, "status" | "delivery" | "sends">) {
  const status = statusView[invitation.status];
  const open = invitation.status === "pending" || invitation.status === "expired" || invitation.status === "needs-resend";
  const delivery = invitation.status === "needs-resend"
    ? { label: "Link no longer works", tone: "warning" as Tone }
    : deliveryView[invitation.delivery];
  return {
    ...status,
    delivery,
    sends: invitation.sends === 1 ? "Sent once" : `Sent ${invitation.sends} times`,
    canResend: open,
    canRevoke: open,
  };
}

/** One line per address after an invite. */
export function invitationResultMessage(result: InvitationResult): { label: string; text: string; tone: Tone } {
  switch (result.status) {
    case "invited": {
      const sent = result.delivery === "queued" ? "Invitation email handed to mail." : result.delivery === "failed" ? "The invitation exists but its email failed. Resend it." : "The invitation exists but no email left. Resend it.";
      const ok = result.delivery === "queued" && result.acceptable;
      return { label: ok ? "Invited" : "Check", text: result.acceptable ? sent : `${sent} The allowlist does not include this address; add it or choose Invitation only.`, tone: ok ? "success" : "warning" };
    }
    case "already-invited":
      return { label: "Already invited", text: "Resend from the list to send a new link.", tone: "neutral" };
    case "already-member":
      return { label: "Has an app account", text: "Already has an active app account.", tone: "neutral" };
    case "suspended":
      return { label: "Suspended", text: "This app account is suspended. Resume it instead.", tone: "warning" };
    case "invalid-email":
      return { label: "Not valid", text: "Not a valid email address.", tone: "danger" };
  }
}

/** Whether the installation can send account email now, and why not. */
export function mailReadiness(status: MailStatus | null): { ready: boolean; reason: string } {
  if (!status) return { ready: false, reason: "Checking whether this installation can send email." };
  if (status.sending) return { ready: true, reason: "" };
  if (!status.transport) return { ready: false, reason: "This installation has no mail transport, so it cannot send invitations. The operator sets LITTLEBOX_MAIL_TRANSPORT." };
  if (status.preflight?.state === "checking") return { ready: false, reason: "This installation is still checking its mail sender. Invitations wait until it passes." };
  const why = status.preflight?.reason ? ` ${status.preflight.reason.replace(/\.?$/, ".")}` : "";
  return { ready: false, reason: `This installation cannot send email: the provider refused the sender.${why}` };
}

/** Kernel times for accounts and invitations are Unix seconds. */
export function formatDate(seconds: number, locale?: string) {
  return new Date(seconds * 1000).toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
}

export function expiryText(invitation: Pick<AppInvitation, "status" | "expiresAt" | "acceptedAt" | "revokedAt">, nowSeconds = Date.now() / 1000, locale?: string) {
  if (invitation.status === "accepted" && invitation.acceptedAt) return `Accepted ${formatDate(invitation.acceptedAt, locale)}`;
  if (invitation.status === "revoked" && invitation.revokedAt) return `Revoked ${formatDate(invitation.revokedAt, locale)}`;
  if (invitation.status === "already-member" || invitation.status === "needs-resend") return "";
  if (invitation.expiresAt <= nowSeconds) return `Expired ${formatDate(invitation.expiresAt, locale)}`;
  return `Expires ${formatDate(invitation.expiresAt, locale)}`;
}

// Integrations

export type RouteAction = { name: string; method: "GET" | "POST"; path: string; signature?: { header: string; secret: string }; inputs: Array<{ key: string; type: string }> };

function actionsOf(source: string | undefined): ServerActionDefinition[] {
  if (!source) return [];
  try {
    return Object.values(parseServerActionSource(source).actions);
  } catch {
    return [];
  }
}

/** Actions that answer on an HTTP route, the ones an API key can name. */
export function routeActions(source: string | undefined): RouteAction[] {
  return actionsOf(source).flatMap((action) => action.http ? [{
    name: action.name,
    method: action.http.method,
    path: action.http.path,
    ...(action.http.signature ? { signature: action.http.signature } : {}),
    inputs: action.bindings.flatMap((binding) => binding.source === "input" ? [{ key: binding.key, type: binding.type }] : []),
  }] : []).sort((a, b) => a.name.localeCompare(b.name));
}

/** Which actions in these sources sign with each secret. */
export function secretUsage(source: string | undefined): Map<string, string[]> {
  const usage = new Map<string, string[]>();
  for (const action of routeActions(source)) {
    if (!action.signature) continue;
    usage.set(action.signature.secret, [...(usage.get(action.signature.secret) ?? []), action.name]);
  }
  return usage;
}

export const SECRET_NAME = /^[a-z][a-z0-9_]{0,63}$/;

/** The same checks `secret_set` makes, so the form says why before the Kernel does. */
export function secretProblem(name: string, value: string): string {
  if (!SECRET_NAME.test(name)) return "A secret name uses lowercase letters, digits and \"_\", starting with a letter, up to 64 characters.";
  if (value.length < 16 || value.length > 512 || !/^[\x21-\x7e]+$/.test(value)) return "A secret value is 16 to 512 printable ASCII characters without spaces.";
  return "";
}

const KEY_PLACEHOLDER = "$LITTLEBOX_API_KEY";

function sample(type: string) {
  return type === "number" ? 1 : type === "boolean" ? true : "text";
}

function shellQuote(value: string) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/**
 * A curl command that calls one route with an API key, as docs/mcp.md
 * describes: `Authorization: Bearer lbk_...` on the Deployment host. A key
 * that names a signed action replaces the signature. A GET maps the query
 * string onto the inputs; a POST sends a JSON object.
 */
export function routeCurl(appUrl: string, action: RouteAction, key = KEY_PLACEHOLDER): string {
  const auth = `-H "Authorization: Bearer ${key}"`;
  const base = appUrl.replace(/\/$/, "");
  if (action.method === "GET") {
    const query = new URLSearchParams(action.inputs.map((input) => [input.key, String(sample(input.type))])).toString();
    return `curl ${auth} "${base}${action.path}${query ? `?${query}` : ""}"`;
  }
  const body = JSON.stringify(Object.fromEntries(action.inputs.map((input) => [input.key, sample(input.type)])));
  return `curl -X POST ${auth} -H 'Content-Type: application/json' --data ${shellQuote(body)} "${base}${action.path}"`;
}

/** A curl command for the file read path. The file ID comes from the app's upload. */
export function fileCurl(appUrl: string, key = KEY_PLACEHOLDER): string {
  return `curl -OJ -H "Authorization: Bearer ${key}" "${appUrl.replace(/\/$/, "")}/_littlebox/files/<file id>"`;
}

/**
 * What the Integrations view says about revoked keys. A key that arrived with
 * an imported app is listed apart, with what it could call, because its
 * program stopped working when the app moved and needs a new key here.
 */
export function revokedKeyNotes(keys: Pick<ApiKey, "name" | "actions" | "files" | "revokedAt" | "revokedReason">[]): string[] {
  const revoked = keys.filter((key) => key.revokedAt);
  const imported = revoked.filter((key) => key.revokedReason === "import");
  const byOwner = revoked.filter((key) => key.revokedReason !== "import");
  const notes: string[] = [];
  if (byOwner.length) notes.push(`${byOwner.length === 1 ? "1 revoked key" : `${byOwner.length} revoked keys`}: ${byOwner.map((key) => key.name).join(", ")}.`);
  if (imported.length) {
    const scope = (key: (typeof imported)[number]) => [...key.actions, ...key.files.map((visibility) => `${visibility} files`)].join(", ");
    notes.push(`Revoked at import, because keys do not work on another machine: ${imported.map((key) => `${key.name} (${scope(key)})`).join("; ")}. Create a new key for each program that still needs access.`);
  }
  return notes;
}

export function apiKeyProblem(name: string, actions: string[], files: string[]): string {
  if (!name.trim()) return "Name the key so you can tell it apart later.";
  if (name.trim().length > 80) return "A key name is 1 to 80 characters.";
  if (actions.length === 0 && files.length === 0) return "Choose at least one route or file visibility for the key.";
  return "";
}
