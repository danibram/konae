import type { AgentTool } from "./contracts";

/** One entry of the Kernel's `tools/list`, as the owner's session reads it. */
export type McpToolDefinition = {
  name: string;
  description?: string;
  inputSchema?: { type?: string; properties?: Record<string, unknown>; required?: string[]; [key: string]: unknown };
};

/** What the editor shows the owner before the agent runs a tool that leaves the project or cannot be undone. */
export type PlatformConfirmation = {
  projectId: string;
  tool: string;
  /** One sentence naming the effect, for the dialog. */
  summary: string;
  arguments: Record<string, unknown>;
};

export type PublishOutcome =
  | { status: "published"; url: string; visibility: "public" | "private" }
  | { status: "declined" }
  | { status: "failed"; error: string };

/**
 * The editor's side of ADR-0022: the owner's MCP session, which deployment the
 * open project has, a confirmation dialog and the Deploy dialog.
 */
export type AgentPlatformBinding = {
  listTools(signal?: AbortSignal): Promise<McpToolDefinition[]>;
  call(name: string, args: Record<string, unknown>, signal?: AbortSignal): Promise<unknown>;
  deploymentForProject(projectId: string): string | null;
  confirm(request: PlatformConfirmation): Promise<boolean>;
  /** Opens the Deploy dialog, where the owner chooses visibility and who may sign up, and resolves when it closes. */
  publish?(projectId: string): Promise<PublishOutcome>;
  /** Called after a confirmed tool ran, so the editor can refresh what it shows. */
  onChanged?(tool: string): void;
};

export type PlatformToolContext = {
  projectId: string;
  /** A tool from an abandoned run cannot act on a newly selected project. */
  isCurrent(): boolean;
  binding: AgentPlatformBinding;
};

type Policy = "read" | "confirm";

/**
 * The tools the build agent gets from the owner's MCP, and whether each asks the
 * owner first. A tool the Kernel lists that is not here never reaches the model:
 * transfers, credentials, secret values, exports and migration stay with
 * the owner. `project_sources_read` and `project_sources_save` are left out on
 * purpose, because in the editor the agent edits a scratch copy behind the
 * Validation Gate and Apply saves it.
 */
export const platformToolPolicy: Readonly<Record<string, Policy>> = {
  session_inspect: "read",
  platform_guide: "read",
  project_inspect: "read",
  database_execute: "read",
  usage_summary: "read",
  notices_read: "read",
  signup_policy_read: "read",
  sign_in_methods_read: "read",
  app_users_list: "read",
  domain_read: "read",
  path_read: "read",
  signup_policy_set: "confirm",
  sign_in_methods_set: "confirm",
  app_invitations_create: "confirm",
  domain_set: "confirm",
  domain_clear: "confirm",
  path_set: "confirm",
  path_clear: "confirm",
};

const readOnlyDatabaseOperations = new Set(["inspect", "browse", "list-backups"]);

/** database_execute reads for inspect, browse, list-backups and a single SELECT; anything else changes the live data. */
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

export function describeCall(name: string, args: Record<string, unknown>): string {
  switch (name) {
    case "signup_policy_set": return `The agent will change who may create an account on this app to ${String(args.mode)}.`;
    case "sign_in_methods_set": return `The agent will set how people sign in to this app to ${Array.isArray(args.methods) ? args.methods.join(" and ") : String(args.methods)}.`;
    case "app_invitations_create": {
      const emails = Array.isArray(args.emails) ? args.emails.map(String) : [];
      return `The agent will email an invitation to ${emails.join(", ") || "nobody"}.`;
    }
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

const pinned = ["projectId", "deploymentId"] as const;

/** The schema the model sees: the project and deployment are the open project's, so the model never names one. */
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

/** A refusal keeps the Kernel's words and code; a lost session says to sign in, not that the tool broke. */
function explain(error: unknown): Error {
  const status = (error as { status?: unknown } | null)?.status;
  if (status === 401) return new Error("The owner's session ended. Ask the owner to sign in again, then retry. Nothing was changed.");
  const refusal = (error as { refusal?: { code?: string; hint?: { fix?: string } } } | null)?.refusal;
  const message = error instanceof Error ? error.message : String(error);
  return new Error(refusal?.code ? `${message} (${refusal.code})${refusal.hint?.fix ? ` ${refusal.hint.fix}` : ""}` : message);
}

/**
 * The owner's MCP tools this agent may call, read from the session that is
 * signed in now. Each call names the open project's project and deployment,
 * and a call that would change something outside the project, send email or
 * change who may get in waits for the owner.
 */
export async function createPlatformTools(context: PlatformToolContext, signal?: AbortSignal): Promise<AgentTool<any>[]> {
  const { binding, projectId } = context;
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
    const takesDeployment = "deploymentId" in properties;
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
        if (takesDeployment) {
          const deploymentId = binding.deploymentForProject(projectId);
          if (!deploymentId) throw new Error(`Publish this project first. ${tool.name} reads or changes the published app.`);
          args.deploymentId = deploymentId;
        }
        const confirm = needsConfirmation(tool.name, given);
        if (confirm) {
          const approved = await binding.confirm({ projectId, tool: tool.name, summary: describeCall(tool.name, given), arguments: given });
          check(signal);
          if (!approved) throw new Error(`The owner declined ${tool.name}. Nothing was changed.`);
          if (takesDeployment && binding.deploymentForProject(projectId) !== args.deploymentId) {
            throw new Error("This project's deployment changed while waiting for the owner. Read it again before changing it.");
          }
        }
        let result: unknown;
        try {
          result = await binding.call(tool.name, args, signal);
        } catch (error) {
          throw explain(error);
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
        const outcome = await publish(projectId);
        check(signal);
        if (outcome.status === "declined") throw new Error("The owner closed the Deploy dialog without publishing.");
        if (outcome.status === "failed") throw new Error(`Publishing was refused: ${outcome.error}`);
        binding.onChanged?.("publish");
        return { content: [{ type: "text" as const, text: `Published as a ${outcome.visibility} app at ${outcome.url}. Open it and call one Server Action to see that it works.` }], details: outcome };
      },
    };
  }
}
