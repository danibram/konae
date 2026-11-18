import type { AgentTool } from "./contracts";
import { sdkContract, type SdkModuleOption } from "../sdk/contract";

export type McpToolDefinition = {
  name: string;
  description?: string;
  inputSchema?: { type?: string; properties?: Record<string, unknown>; required?: string[]; [key: string]: unknown };
};

export type PlatformConfirmation = {
  projectId: string;
  tool: string;
  summary: string;
  arguments: Record<string, unknown>;
};

export type PublishOutcome =
  | { status: "published"; url: string; visibility: "public" | "private" }
  | { status: "declined" }
  | { status: "failed"; error: string };

export type AgentPlatformBinding = {
  listTools(signal?: AbortSignal): Promise<McpToolDefinition[]>;
  call(name: string, args: Record<string, unknown>, signal?: AbortSignal): Promise<unknown>;
  callAgent?(name: string, args: Record<string, unknown>, callId: string, signal?: AbortSignal, observesCallId?: string): Promise<unknown>;
  deploymentForProject(projectId: string): string | null;
  confirm(request: PlatformConfirmation): Promise<boolean>;
  publish?(projectId: string, callId?: string): Promise<PublishOutcome>;
  onChanged?(tool: string): void;
};

export type PlatformToolContext = {
  projectId: string;
  isCurrent(): boolean;
  binding: AgentPlatformBinding;
  refreshSession?: boolean;
  onCallBoundary?(callId: string, boundary: { arguments: Record<string, unknown>; effectiveArguments: Record<string, unknown>; confirmation: "not-required" | "requested" | "approved" | "declined" }): void;
};

type Policy = "read" | "confirm";

export const platformToolPolicy: Readonly<Record<string, Policy>> = {
  session_inspect: "read",
  platform_guide: "read",
  project_inspect: "read",
  database_execute: "read",
  usage_summary: "read",
  notices_read: "read",
  app_users_list: "read",
  domain_read: "read",
  path_read: "read",
  modules_read: "read",
  app_invitations_create: "confirm",
  hostname_set: "confirm",
  domain_set: "confirm",
  domain_clear: "confirm",
  path_set: "confirm",
  path_clear: "confirm",
  module_set: "confirm",
};

const readOnlyDatabaseOperations = new Set(["inspect", "browse", "list-backups"]);

export function databaseCallWrites(args: Record<string, unknown>): boolean {
  const operation = args.operation;
  if (typeof operation === "string" && readOnlyDatabaseOperations.has(operation)) return false;
  if (operation !== "sql") return true;
  const sql = (args.input as { sql?: unknown } | undefined)?.sql;
  if (typeof sql !== "string") return true;
  const statement = sql.trim().replace(/;\s*$/, "");
  return !/^select\b/i.test(statement) || statement.includes(";");
}

function needsConfirmation(name: string, args: Record<string, unknown>): boolean {
  if (name === "database_execute") return databaseCallWrites(args);
  return platformToolPolicy[name] !== "read";
}

function requiredScope(name: string, args: Record<string, unknown>): string {
  if (name === "project_export") return "export_data";
  if (["app_invitations_create", "app_invitation_resend", "app_invitation_revoke", "app_user_suspend", "app_user_resume", "app_user_enter"].includes(name)) return "manage_users";
  if (name === "module_set") return sdkContract.modules.find((item) => item.name === String(args.module))?.permission ?? "publish";
  if (["release_apply", "project_import", "api_keys_create", "api_keys_revoke", "hostname_set", "domain_set", "domain_clear", "path_set", "path_clear"].includes(name)) return "publish";
  return needsConfirmation(name, args) ? "write" : "read";
}

export function describeCall(name: string, args: Record<string, unknown>): string {
  switch (name) {
    case "app_invitations_create": {
      const emails = Array.isArray(args.emails) ? args.emails.map(String) : [];
      return `The agent will email an invitation to ${emails.join(", ") || "nobody"}.`;
    }
    case "module_set": {
      const name = String(args.module);
      const module = sdkContract.modules.find((item) => item.name === name);
      const change = typeof args.on === "boolean" ? `turn ${args.on ? "on" : "off"}` : "update";
      const options = args.options && typeof args.options === "object" ? Object.entries(args.options as Record<string, unknown>).map(([key, value]) => {
        const option = module?.options.find((item) => item.key === key) as SdkModuleOption | undefined;
        let label = option?.valueLabels?.[String(value)] ?? String(value);
        if (option?.type === "signup-policy" && value && typeof value === "object") {
          const policy = value as { mode?: unknown; allowlist?: unknown };
          label = option.valueLabels?.[String(policy.mode)] ?? String(policy.mode);
          if (policy.mode === "allowlist" && Array.isArray(policy.allowlist) && policy.allowlist.length) label += ` (${policy.allowlist.join(", ")})`;
        } else if (option?.type === "sign-in-methods" && Array.isArray(value)) {
          label = value.map((method) => option.valueLabels?.[String(method)] ?? String(method)).join(" and ");
        }
        return `${option?.label ?? key} to ${label}`;
      }) : [];
      return `The agent will ${change} ${module?.title ?? name} (${name})${options.length ? ` and set ${options.join(", ")}` : ""}.`;
    }
    case "hostname_set": return `The agent will move this app's generated address to ${String(args.name)}. The old address redirects there, and app users sign in again on the new one.`;
    case "domain_set": return `The agent will set ${String(args.hostname)} as this app's domain and check its TXT record.`;
    case "domain_clear": return "The agent will remove this app's custom domain. The generated address keeps working.";
    case "path_set": {
      const turn = args.enabled === true ? "turn on" : "turn off";
      const named = typeof args.name === "string" ? ` with the name ${args.name}` : args.name === null ? " and remove its name" : "";
      const shared = args.acceptSharedOrigin === true ? " The path shares its origin with the home and every other app on this installation, so they can use this app's signed-in sessions." : "";
      return `The agent will ${turn} this app's path address${named}.${shared}`;
    }
    case "path_clear": return "The agent will turn off this app's path address and remove its name. The generated address and any custom domain keep working.";
    case "database_execute": return args.operation === "sql"
      ? `The agent will run this SQL on the published app's data: ${String((args.input as { sql?: unknown } | undefined)?.sql ?? "")}`
      : `The agent will ${String(args.operation)} on the published app's data.`;
    default: return `The agent will run ${name}.`;
  }
}

const pinned = ["projectId"] as const;
const publishedProjectTools = new Set([
  "database_execute", "api_keys_list", "domain_read", "path_read", "maintenance_read", "contact_email_read",
  "project_export", "calls_read", "notices_read", "usage_summary", "signals_read",
  "app_user_enter", "app_users_list", "app_user_suspend",
  "app_user_resume", "app_invitations_create", "app_invitations_list", "app_invitation_resend",
  "app_invitation_revoke", "api_keys_create", "api_keys_revoke", "project_import", "contact_email_set",
  "maintenance_set", "hostname_set", "domain_set", "domain_clear", "path_set", "path_clear",
  "modules_read", "module_set",
]);

function modelSchema(schema: McpToolDefinition["inputSchema"]) {
  const properties = { ...(schema?.properties ?? {}) };
  for (const key of pinned) delete properties[key];
  const required = (schema?.required ?? []).filter((key) => !(pinned as readonly string[]).includes(key));
  const { required: _dropped, ...rest } = schema ?? {};
  return { ...rest, type: "object", properties, ...(required.length ? { required } : {}) };
}

const maxResultCharacters = 20_000;

function text(value: unknown): string {
  const body = typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
  return body.length > maxResultCharacters ? `${body.slice(0, maxResultCharacters)}\n… cut at ${maxResultCharacters} characters. Ask for less, such as a smaller page or a narrower query.` : body;
}

function explain(error: unknown): Error {
  const status = (error as { status?: unknown } | null)?.status;
  if (status === 401) return new Error("The owner's session ended. Ask the owner to sign in again, then retry. Nothing was changed.");
  const refusal = (error as { refusal?: { code?: string; hint?: { fix?: string } } } | null)?.refusal;
  const message = error instanceof Error ? error.message : String(error);
  return new Error(refusal?.code ? `${message} (${refusal.code})${refusal.hint?.fix ? ` ${refusal.hint.fix}` : ""}` : message);
}

export async function createPlatformTools(context: PlatformToolContext, signal?: AbortSignal): Promise<AgentTool<any>[]> {
  const { binding, projectId } = context;
  let readBackMap: Record<string,string> = {};
  if (context.refreshSession) {
    // This bootstrap runs while Durable is hydrating and before openStorage
    // claims the project lease. These are authenticated reads; the execution
    // identity is attached to every later agent operation.
    await binding.call("session_inspect", {}, signal);
    const guide = await binding.call("platform_guide", { topic: "rules" }, signal) as { readBack?: { map?: Record<string, unknown> } };
    readBackMap = Object.fromEntries(Object.entries(guide.readBack?.map ?? {}).filter((entry): entry is [string,string] => typeof entry[1] === "string"));
  }
  const listed = await binding.listTools(signal);
  const tools = listed
    .filter((tool) => tool.name in platformToolPolicy)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(platformTool);
  if (binding.publish) tools.push(publishTool(binding.publish));
  return tools;

  function check(signal?: AbortSignal) {
    if (signal?.aborted || !context.isCurrent()) throw new Error("The current project or agent run changed. Ask again in the current project.");
  }

  function platformTool(tool: McpToolDefinition): AgentTool<any> {
    const properties = tool.inputSchema?.properties ?? {};
    const takesProject = "projectId" in properties;
    return {
      name: tool.name,
      label: tool.name.replaceAll("_", " "),
      description: `${tool.description ?? tool.name} The open project and its deployment are filled in for you.`,
      parameters: modelSchema(tool.inputSchema) as never,
      executionMode: needsConfirmationAtAll(tool.name) ? "sequential" : undefined,
      execute: async (_id, params, signal) => {
        check(signal);
        const given = { ...((params ?? {}) as Record<string, unknown>) };
        for (const key of pinned) {
          if (key in given) throw new Error(`${tool.name} acts on the open project only. Leave out ${key}.`);
        }
        const args: Record<string, unknown> = { ...given };
        if (takesProject) args.projectId = projectId;
        const needsDeployment = publishedProjectTools.has(tool.name);
        if (needsDeployment && !binding.deploymentForProject(projectId)) {
          throw new Error(`Publish this project first. ${tool.name} reads or changes the published app.`);
        }
        const confirm = needsConfirmation(tool.name, given);
        if (confirm) {
          if (tool.name === "module_set") {
            const module = sdkContract.modules.find((item) => item.name === String(given.module));
            if (!module || module.agentRule === "never") throw new Error(`The ${String(given.module)} module cannot be changed by the build agent.`);
          }
          context.onCallBoundary?.(_id, { arguments: given, effectiveArguments: args, confirmation: "requested" });
          const approved = await binding.confirm({ projectId, tool: tool.name, summary: describeCall(tool.name, given), arguments: given });
          check(signal);
          if (!approved) {
            context.onCallBoundary?.(_id, { arguments: given, effectiveArguments: args, confirmation: "declined" });
            throw new Error(`The owner declined ${tool.name}. Nothing was changed.`);
          }
          context.onCallBoundary?.(_id, { arguments: given, effectiveArguments: args, confirmation: "approved" });
          if (needsDeployment && !binding.deploymentForProject(projectId)) {
            throw new Error("This project's deployment changed while waiting for the owner. Read it again before changing it.");
          }
        }
        if (!confirm) context.onCallBoundary?.(_id, { arguments: given, effectiveArguments: args, confirmation: "not-required" });
        if (binding.callAgent && needsConfirmation(tool.name, given)) {
          const inspect = await binding.callAgent("session_inspect", {}, `${_id}-permissions`, signal);
          const session = inspect as { can?: Record<string, unknown>; projectIds?: unknown; ownerId?: unknown };
          const scope = requiredScope(tool.name, given);
          if (session.can?.[scope] !== true) throw new Error(`Permission changed while the confirmation was open. session_inspect.can.${scope} is not granted; ${tool.name} was not sent.`);
          if (Array.isArray(session.projectIds) && session.projectIds.length > 0 && !session.projectIds.includes(projectId)) throw new Error(`Project access changed while the confirmation was open. ${tool.name} was not sent.`);
        }
        let result: unknown;
        try {
          result = await (binding.callAgent ? binding.callAgent(tool.name, args, _id, signal) : binding.call(tool.name, args, signal));
        } catch (error) {
          if (tool.name === "module_set" && (error as { refusal?: { code?: string } } | null)?.refusal?.code === "module.changed") {
            const current = await (binding.callAgent ? binding.callAgent("modules_read", { projectId }, `${_id}-readback`, signal, _id) : binding.call("modules_read", { projectId }, signal)).catch(() => null);
            throw new Error(`${explain(error).message}${current ? ` Current modules_read state: ${text(current)}` : ""}`);
          }
          const transport = error as { status?: number; refusal?: unknown } | null;
          if (binding.callAgent && confirm && transport?.status === undefined && !transport?.refusal) {
            throw new Error(`The response to ${tool.name} was lost after dispatch may have begun. Its outcome is unknown. Read it back through platform_guide rules before taking a deliberate follow-up; Konae will not replay it.`);
          }
          throw explain(error);
        }
        const writeResponse = result;
        if (tool.name === "module_set") {
          try { result = await (binding.callAgent ? binding.callAgent("modules_read", { projectId }, `${_id}-readback`, signal, _id) : binding.call("modules_read", { projectId }, signal)); }
          catch (error) { result = { outcome: "unknown", writeResponse, message: "The write response arrived, but its documented modules_read read-back failed. The write has not been verified; do not repeat it until its outcome is reconciled.", error: (error as { status?: number } | null)?.status === 401 ? "The owner session expired during read-back." : explain(error).message }; }
          if (result && typeof result === "object" && (result as { outcome?: string }).outcome !== "unknown") result = { outcome: "observed", writeResponse, readBackTool: "modules_read", observation: result };
        } else if (binding.callAgent && needsConfirmation(tool.name, given)) {
          const readTool = readBackMap[tool.name];
          if (readTool && readTool !== "database_execute") {
            try {
              const readArgs: Record<string,unknown> = {};
              if (readTool !== "auth_session" && readTool !== "mail_status") readArgs.projectId = projectId;
              const observation = await binding.callAgent(readTool, readArgs, `${_id}-readback`, signal, _id);
              result = { outcome: "observed", writeResponse, readBackTool: readTool, observation };
            } catch (error) {
              result = { outcome: "unknown", writeResponse, message: `The write response arrived, but the documented ${readTool} read-back failed. The write has not been verified; do not repeat it until its outcome is reconciled.`, error: (error as { status?: number } | null)?.status === 401 ? "The owner session expired during read-back." : explain(error).message };
            }
          }
        }
        check(signal);
        if (confirm) binding.onChanged?.(tool.name);
        return { content: [{ type: "text" as const, text: text(result) }], details: result };
      },
    };
  }

  function needsConfirmationAtAll(name: string) {
    return name === "database_execute" || platformToolPolicy[name] === "confirm";
  }

  function publishTool(publish: NonNullable<AgentPlatformBinding["publish"]>): AgentTool<any> {
    return {
      name: "publish",
      label: "Publish",
      executionMode: "sequential",
      description: "Only when the owner asks to publish, open the Deploy dialog for the open project. The owner chooses who may open the app and who may create an account there, and presses Deploy. Apply your edits first: the dialog publishes the project as saved, not your scratch copy. The result says whether it was published, declined or refused.",
      parameters: { type: "object", properties: {}, additionalProperties: false } as never,
      execute: async (_id, _params, signal) => {
        check(signal);
      const outcome = await publish(projectId, _id);
        check(signal);
        context.onCallBoundary?.(_id, { arguments: {}, effectiveArguments: { projectId }, confirmation: outcome.status === "published" ? "approved" : "declined" });
        if (outcome.status === "declined") throw new Error("The owner closed the Deploy dialog without publishing.");
        if (outcome.status === "failed") throw new Error(`Publishing was refused: ${outcome.error}`);
        binding.onChanged?.("publish");
        return { content: [{ type: "text" as const, text: `Published as a ${outcome.visibility} app at ${outcome.url}. Open it and call one Server Action to see that it works.` }], details: outcome };
      },
    };
  }
}
