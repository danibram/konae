import documentsGuide from "../../docs/documents.md?raw";
import fileIntakeGuide from "../../docs/file-intake.md?raw";
import ftpsGuide from "../../docs/ftps.md?raw";
import appMcpGuide from "../../docs/mcp-app-tools.md?raw";
import schemaTransformsGuide from "../../docs/guides/schema-transformations.md?raw";
import { sdkReferences } from "./sdk-section.generated";
import type { PreviewTest, PreviewTestReport } from "./preview-test";
import { editJson, type JsonChange } from "./json-edit";
import { Type, type TSchema } from "@earendil-works/pi-ai";
import type { FileError } from "@earendil-works/pi-agent-core";
import type { ProjectFile } from "../lib/types";
import { capsuleFileFor, capsulePathFor, isTemplatePath, readTemplateFile } from "./capsule-env";
import type {
  AgentTool,
  AgentToolResult,
  CapsuleFiles,
  CapsuleValidator,
  CapsuleWorkspace,
  LiveDatabase,
  LiveDatabaseExport,
  Result,
  ValidationResult,
} from "./contracts";
import type { ReleaseFacts } from "./release-check";
import type { ValidationFacts } from "./contracts";
import type { SourceFiles } from "../lib/source-files";
import { refusalOf, type Refusal } from "../kernel/refusal";

const references: Record<string, string> = {
  ...sdkReferences,
  "/reference/schema-transforms.md": schemaTransformsGuide,
  "/reference/documents.md": documentsGuide,
  "/reference/file-intake.md": fileIntakeGuide,
  "/reference/ftps.md": ftpsGuide,
  "/reference/mcp-app-tools.md": appMcpGuide,
};
const jsonDeclarations = ["actions.json", "functions.json", "crons.json", "file-intake.json"];
const gateFiles = ["schema.sql", ...jsonDeclarations];
const gatePaths = gateFiles.map((file) => `/workspace/${file}`).join(", ");

export type PreviewFailureReport = {
  kind: string;
  message: string;
  stack: string;
  count: number;
};

export type CapsuleToolHooks = {
  onValidated?: (files: CapsuleFiles, result: ValidationResult) => void;
  liveDatabase?: LiveDatabaseExport;
  previewErrors?: () => PreviewFailureReport[];
  previewTest?: (files: CapsuleFiles, test: PreviewTest, signal?: AbortSignal) => Promise<PreviewTestReport>;
  releaseFacts?: () => Promise<ReleaseFacts>;
  sourceFiles?: () => SourceFiles;
  functionPreparation?: (files: CapsuleFiles) => Promise<{ status: "accepted"; scratchRevision: string; buildFingerprint: string }>;
};

export function createCapsuleTools(
  workspace: CapsuleWorkspace,
  validator: CapsuleValidator,
  hooks: CapsuleToolHooks = {},
): AgentTool<any>[] {
  const queue = createQueue();
  return [
    readTool(workspace),
    editTool(workspace, queue),
    editJsonTool(workspace, queue),
    writeTool(workspace, queue),
    validateTool(workspace, validator, queue, hooks),
    previewTestTool(workspace, queue, hooks),
    previewErrorsTool(workspace, queue, hooks),
  ];
}

function readTool(workspace: CapsuleWorkspace): AgentTool<any> {
  return defineTool({
    name: "read",
    label: "Read file",
    description:
      "Read a Capsule file, /reference/sdk/<module>.md, a guide named in the prompt, or a template. Current files were included in the first message, " +
      "so read only to re-check a file after several edits, or to fetch a line range of a long file. " +
      "Use offset/limit to read part of a file; omit both to read all of it. " +
      "The seven templates are read-only under /templates/<id>/ (tasks, notes, members, form, webhook, digest, gallery), " +
      "each with index.html, styles.css, app.js, schema.sql and actions.json that already pass the Gate. " +
      "Read the matching template's files before rebuilding a Capsule in its shape.",
    parameters: Type.Object({
      path: Type.String({ description: "Capsule file path, for example /workspace/app.js, or a template file such as /templates/webhook/actions.json" }),
      offset: Type.Optional(Type.Integer({ minimum: 1, description: "First line to return, 1-based. Defaults to 1." })),
      limit: Type.Optional(Type.Integer({ minimum: 1, description: "Maximum number of lines to return." })),
    }),
    async execute(_toolCallId, params: { path: string; offset?: number; limit?: number }, signal?: AbortSignal) {
      const reference = Object.hasOwn(references, params.path);
      const template = isTemplatePath(params.path);
      const file = template || reference ? params.path : requireCapsuleFile(params.path);
      const content = reference ? references[params.path] : template ? unwrap(readTemplateFile(params.path)) : unwrap(await workspace.env.readTextFile(capsulePathFor(file), signal));
      const lines = content.split("\n");
      const start = Math.max((params.offset ?? 1) - 1, 0);
      const end = params.limit === undefined ? lines.length : Math.min(start + params.limit, lines.length);
      const slice = lines.slice(start, end);
      const partial = start > 0 || end < lines.length;
      const header = partial ? `${file} lines ${start + 1}-${end} of ${lines.length}\n` : "";
      return result(header + slice.join("\n"), {
        path: template || reference ? file : capsulePathFor(file),
        startLine: start + 1,
        endLine: end,
        totalLines: lines.length,
      });
    },
  });
}

function editTool(workspace: CapsuleWorkspace, queue: Queue): AgentTool<any> {
  return defineTool({
    name: "edit",
    label: "Edit file",
    executionMode: "sequential",
    description:
      "Replace one exact snippet in a Capsule file. This is the preferred way to change a file: keep edits small and targeted " +
      "instead of rewriting whole files. old_string is matched literally, including indentation and newlines, and must appear " +
      "exactly once. Add surrounding lines until it is unique. " +
      `Call validate_capsule after editing any of these files: ${gatePaths}.`,
    parameters: Type.Object({
      path: Type.String({ description: "Capsule file path, for example /workspace/styles.css" }),
      old_string: Type.String({ description: "Exact text to replace. Must occur exactly once in the file." }),
      new_string: Type.String({ description: "Replacement text. Empty to delete the snippet." }),
    }),
    async execute(_toolCallId, params: { path: string; old_string: string; new_string: string }, signal?: AbortSignal) {
      const file = requireCapsuleFile(params.path);
      if (params.old_string === "") throw new Error("old_string cannot be empty. Use write to replace a whole file.");
      if (params.old_string === params.new_string) throw new Error("old_string and new_string are identical.");

      return queue(async () => {
        const content = unwrap(await workspace.env.readTextFile(capsulePathFor(file), signal));
        const found = findSnippet(content, params.old_string);
        if (found.matches === 0) {
          throw new Error(`old_string was not found in ${file}. Read the file and copy the exact text, including indentation.`);
        }
        if (found.matches > 1) {
          throw new Error(`old_string matches ${found.matches} times in ${file}. Include more surrounding lines to make it unique.`);
        }

        const replacement = found.needle === params.old_string ? params.new_string : toCrlf(params.new_string);
        const next = content.slice(0, found.index) + replacement + content.slice(found.index + found.needle.length);
        unwrap(await workspace.env.writeFile(capsulePathFor(file), next, signal));

        const firstChangedLine = content.slice(0, found.index).split("\n").length;
        return result(`Edited ${file} at line ${firstChangedLine}.${validationReminder(file)}`, {
          path: capsulePathFor(file),
          firstChangedLine,
        });
      });
    },
  });
}

function editJsonTool(workspace: CapsuleWorkspace, queue: Queue): AgentTool<any> {
  return defineTool({
    name: "edit_json", label: "Edit JSON", executionMode: "sequential",
    description: "Change JSON by exact JSON Pointer, such as /crons/digest/cron or /actions/clients.create/bindings/2/nullable. All changes succeed together or nothing is written. For add, the key must be missing. For replace and remove, pass the current value as expected. " +
      `Call validate_capsule after changing any of these files: ${jsonDeclarations.join(", ")}.`,
    parameters: Type.Object({
      path: Type.String(),
      changes: Type.Array(Type.Object({
        op: Type.Union([Type.Literal("add"), Type.Literal("replace"), Type.Literal("remove")]),
        pointer: Type.String(), value: Type.Optional(Type.Unknown()), expected: Type.Optional(Type.Unknown()),
      }), { minItems: 1, maxItems: 50 }),
    }),
    async execute(_id, params: {path: string; changes: JsonChange[]}, signal?: AbortSignal) {
      const file = requireCapsuleFile(params.path);
      if (!file.endsWith(".json")) throw new Error("edit_json edits .json files only.");
      return queue(async () => {
        const source = unwrap(await workspace.env.readTextFile(capsulePathFor(file), signal));
        const next = editJson(source, params.changes);
        unwrap(await workspace.env.writeFile(capsulePathFor(file), next, signal));
        return result(`Edited ${file} at ${params.changes.length} JSON paths.${validationReminder(file)}`, { path: capsulePathFor(file), pointers: params.changes.map((change) => change.pointer) });
      });
    },
  });
}

function writeTool(workspace: CapsuleWorkspace, queue: Queue): AgentTool<any> {
  return defineTool({
    name: "write",
    label: "Write file",
    executionMode: "sequential",
    description:
      "Replace an existing Capsule file or create a new one at any nested path inside /workspace. " +
      "Prefer edit unless you are genuinely rewriting the file, because a full rewrite is slow and easily drops unrelated details. " +
      `Call validate_capsule after writing any of these files: ${gatePaths}.`,
    parameters: Type.Object({
      path: Type.String({ description: "Capsule file path, for example /workspace/index.html" }),
      content: Type.String({ description: "The complete new contents of the file." }),
    }),
    async execute(_toolCallId, params: { path: string; content: string }, signal?: AbortSignal) {
      const file = requireCapsuleFile(params.path);
      return queue(async () => {
        unwrap(await workspace.env.writeFile(capsulePathFor(file), params.content, signal));
        const lines = params.content.split("\n").length;
        return result(`Wrote ${file} (${lines} lines).${validationReminder(file)}`, {
          path: capsulePathFor(file),
          lines,
        });
      });
    },
  });
}

function previewTestTool(workspace: CapsuleWorkspace, queue: Queue, hooks: CapsuleToolHooks): AgentTool<any> {
  return defineTool({
    name: "preview_test", label: "Test candidate", executionMode: "sequential",
    description: "Build and run the current scratch files in an isolated browser preview with copied test rows. Fill fields, click buttons, assert text and call named actions as guest, alice or bob. Writes are discarded. Returns the candidate revision, each step, actual action inputs/results, handled action errors, runtime errors, page text and viewport overflow. Use a form submission and a read-back action to verify a changed save flow. It does not test a published Kernel, email, routes, schedules, Google or the published CSP. Network fetches and form navigation are blocked; downloads are not verified; confirm dialogs are accepted in this test only.",
    parameters: Type.Object({
      actor: Type.Optional(Type.Union([Type.Literal("guest"), Type.Literal("alice"), Type.Literal("bob")])),
      width: Type.Optional(Type.Integer({ minimum: 320, maximum: 1920 })),
      steps: Type.Array(Type.Union([
        Type.Object({ op: Type.Literal("fill"), selector: Type.String(), value: Type.String() }),
        Type.Object({ op: Type.Literal("click"), selector: Type.String() }),
        Type.Object({ op: Type.Literal("text"), selector: Type.String(), contains: Type.String() }),
        Type.Object({ op: Type.Literal("action"), name: Type.String(), input: Type.Optional(Type.Record(Type.String(), Type.Unknown())), expectedError: Type.Optional(Type.String()) }),
      ]), {minItems:1,maxItems:20}),
    }),
    async execute(_id, test: PreviewTest, signal?: AbortSignal) {
      if (!hooks.previewTest) throw new Error("Candidate browser tests are unavailable in this session. Tell the owner the flow is untested.");
      if (!test.steps.length || test.steps.length > 20) throw new Error("Provide between 1 and 20 test steps.");
      for (const step of test.steps) {
        if (step.op === "action" ? !step.name : !step.selector) throw new Error("An action step needs name; a UI step needs selector.");
        if (step.op === "fill" && step.value === undefined || step.op === "text" && step.contains === undefined) throw new Error("A fill step needs value; a text step needs contains.");
      }
      return queue(async () => {
        const report = await hooks.previewTest!(workspace.serialize().files, test, signal);
        return result(formatPreviewReport(report), report);
      });
    },
  });
}

function previewErrorsTool(workspace: CapsuleWorkspace, queue: Queue, hooks: CapsuleToolHooks): AgentTool<any> {
  return defineTool({
    name: "preview_errors", label: "Preview errors", executionMode: "sequential",
    description: "Build and load the current scratch files in an isolated candidate preview and read its startup errors. Returns the tested revision. An empty error list tests startup only. Use preview_test to exercise a save and read it back; handled form errors require that interaction.",
    parameters: Type.Object({}),
    async execute(_id, _params, signal?: AbortSignal) {
      return queue(async () => {
        if (hooks.previewTest) {
          const report = await hooks.previewTest(workspace.serialize().files, { actor: "alice", steps: [] }, signal);
          return result(formatPreviewReport(report), report);
        }
        const failures = hooks.previewErrors?.();
        if (!failures) return result("Candidate preview unavailable. No current files were executed. The flow is untested.", { scope: "unavailable" });
        return result("Applied preview only; the scratch files were not executed.\n" + (failures.length ? failures.map((failure) => `${failure.kind}: ${failure.message} (x${failure.count})\n${failure.stack}`).join("\n") : "No runtime errors reported by the applied preview."), { scope: "applied", failures });
      });
    },
  });
}

function formatPreviewReport(report: PreviewTestReport): string {
  const summary = `Candidate ${report.revision.slice(0, 12)} ${report.ok ? "passed" : "failed"}: ${report.steps.length} steps as ${report.actor}, ${report.errors.length} runtime errors.`;
  const json = JSON.stringify(report);
  if (json.length <= 18_000) return summary + "\n" + json;
  const bounded = (value: unknown) => {
    const text = JSON.stringify(value);
    return text && text.length > 1000 ? { truncated: true, excerpt: text.slice(0, 1000) } : value;
  };
  return summary + "\n" + JSON.stringify({ ...report, text: report.text.slice(0, 2000), actions: report.actions.slice(-30).map((action) => ({ ...action, result: bounded(action.result), input: bounded(action.input) })), steps: report.steps.map((step) => ({ ...step, result: bounded(step.result) })), truncated: true });
}

function validateTool(
  workspace: CapsuleWorkspace,
  validator: CapsuleValidator,
  queue: Queue,
  hooks: CapsuleToolHooks,
): AgentTool<any> {
  return defineTool({
    name: "validate_capsule",
    label: "Validate capsule",
    executionMode: "sequential",
    description:
      "Run the Validation Gate on the current workspace. It checks that schema.sql can run twice, that actions prepare against the schema, and that scheduled jobs name valid Functions. " +
      `Call it after changing any of these files: ${gateFiles.join(", ")}. Run it again after fixing reported errors. ` +
      "The change cannot be applied to the Project until this passes.",
    parameters: Type.Object({}),
    async execute() {
      return queue(async () => {
        const files = workspace.serialize().files;
        const validation = await validateCapsuleCandidate(validator, files, hooks);
        hooks.onValidated?.(files, validation);
        return result(formatValidation(validation), validation);
      });
    },
  });
}

export async function validateCapsuleCandidate(validator: CapsuleValidator, files: CapsuleFiles, hooks: CapsuleToolHooks = {}) {
  let validation = await validator.validate(files, await readLiveDatabase(hooks), await readValidationFacts(hooks));
  if (validation.ok && files["functions.json"] && hooks.functionPreparation) {
    try {
      validation = { ...validation, functionPreparation: await hooks.functionPreparation(files) };
    } catch (error) {
      const refusal = refusalOf(error) ?? (error && typeof error === "object" && "refusal" in error
        ? (error as { refusal?: Refusal }).refusal
        : null);
      if (refusal?.code && refusal.message && refusal.hint) {
        validation = { ...validation, ok: false, errors: [...validation.errors, { stage: "actions", code: refusal.code as any, message: refusal.message, hint: refusal.hint }] };
      } else validation = { ...validation, functionPreparation: { status: "pending" } };
    }
  }
  return validation;
}

export async function readLiveDatabase(hooks: CapsuleToolHooks): Promise<LiveDatabase> {
  if (!hooks.liveDatabase) return { state: "absent" };
  try {
    const bytes = await hooks.liveDatabase();
    return bytes ? { state: "loaded", bytes } : { state: "absent" };
  } catch (error) {
    return { state: "unreadable", reason: error instanceof Error ? error.message : String(error) };
  }
}

export async function readReleaseFacts(hooks: CapsuleToolHooks): Promise<ReleaseFacts> {
  if (!hooks.releaseFacts) return {};
  try {
    return await hooks.releaseFacts();
  } catch {
    return {};
  }
}

export async function readValidationFacts(hooks: CapsuleToolHooks): Promise<ValidationFacts> {
  return {
    ...await readReleaseFacts(hooks),
    ...(hooks.sourceFiles ? { sourceFiles: hooks.sourceFiles() } : {}),
  };
}

function defineTool<T extends TSchema, D>(definition: AgentTool<T, D>): AgentTool<any> {
  return definition as unknown as AgentTool<any>;
}

export function formatValidation(validation: ValidationResult): string {
  const warnings = (validation.warnings ?? []).map((warning) => {
    const detail = (warning.detail ? `\n  ${warning.detail}` : "") + refusalLine(warning);
    const owner = warning.owner ? " Tell the person which secret to set and that they set it in Access > Integrations; Apply does not wait for it." : "";
    return `\n- [${warning.stage}] Warning: ${warning.message}${detail}${owner}`;
  }).join("");
  if (validation.ok) {
    if (validation.functionPreparation?.status === "pending") return `Validation passed, but function preparation is pending because the owner MCP did not answer. Keep the proposal and reconnect before Apply.${warnings}`;
    const skipped = validation.checked.includes("preview-rows")
      ? ""
      : " The preview-rows stage did not run, so nothing was checked against the live database.";
    return `Validation passed: schema, server actions and bindings are consistent.${skipped}${warnings}`;
  }
  const lines = validation.errors.map((error) => {
    const detail = (error.detail ? `\n  ${error.detail}` : "") + refusalLine(error);
    return `- [${error.stage}] ${error.message}${detail}`;
  });
  return `Validation failed with ${validation.errors.length} problem${validation.errors.length === 1 ? "" : "s"}:\n${lines.join("\n")}${warnings}`;
}

function refusalLine(finding: { code?: string; hint?: { remedy: string } }): string {
  return finding.code ? `\n  Code ${finding.code}: ${finding.hint?.remedy ?? ""}`.trimEnd() : "";
}

function requireCapsuleFile(path: string): ProjectFile {
  const file = capsuleFileFor(path);
  if (!file) throw new Error(`${path} is not a safe Capsule file path. Files must stay inside /workspace.`);
  return file;
}

function validationReminder(file: ProjectFile): string {
  return gateFiles.includes(file) ? " Call validate_capsule now." : "";
}

type Snippet = { matches: number; index: number; needle: string };

function findSnippet(content: string, oldString: string): Snippet {
  const direct = countOccurrences(content, oldString);
  if (direct > 0) return { matches: direct, index: content.indexOf(oldString), needle: oldString };
  if (!oldString.includes("\r") && content.includes("\r\n")) {
    const crlf = toCrlf(oldString);
    const matches = countOccurrences(content, crlf);
    if (matches > 0) return { matches, index: content.indexOf(crlf), needle: crlf };
  }
  return { matches: 0, index: -1, needle: oldString };
}

function toCrlf(text: string): string {
  return text.includes("\r") ? text : text.replaceAll("\n", "\r\n");
}

function countOccurrences(content: string, needle: string): number {
  let count = 0;
  let index = content.indexOf(needle);
  while (index !== -1) {
    count += 1;
    index = content.indexOf(needle, index + 1);
  }
  return count;
}

type Queue = <T>(run: () => Promise<T>) => Promise<T>;

function createQueue(): Queue {
  let tail: Promise<unknown> = Promise.resolve();
  return <T>(run: () => Promise<T>) => {
    const next = tail.then(run, run);
    tail = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };
}

function unwrap<T>(value: Result<T, FileError>): T {
  if (!value.ok) throw new Error(value.error.message);
  return value.value;
}

function result<T>(text: string, details: T): AgentToolResult<T> {
  return { content: [{ type: "text", text }], details };
}
