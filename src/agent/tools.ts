import { Type, type TSchema } from "@earendil-works/pi-ai";
import type { FileError } from "@earendil-works/pi-agent-core";
import type { ProjectFile } from "../lib/types";
import { createDomainTools, type DomainToolContext } from "./domain-tools";
import { capsuleFileFor, capsulePathFor } from "./capsule-env";
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

export type PreviewFailureReport = {
  kind: string;
  message: string;
  stack: string;
  count: number;
};

export type CapsuleToolHooks = {
  domains?: DomainToolContext;
  /** Called with the exact files a `validate_capsule` run saw, so the gate can trust the verdict. */
  onValidated?: (files: CapsuleFiles, result: ValidationResult) => void;
  /** The live database, so the model's own validation runs the same stages the gate does. */
  liveDatabase?: LiveDatabaseExport;
  /** Runtime failures the running preview reported; absent when no preview is attached. */
  previewErrors?: () => PreviewFailureReport[];
};

/**
 * The Capsule tool set: read, write, edit, validate_capsule. No shell, no
 * listing and no search. All current files arrive in the first user message,
 * and `write` may create a new nested source file.
 *
 * Every tool that mutates the workspace, and the validation that reads it, runs
 * through one queue and declares `executionMode: "sequential"`: a single
 * assistant message may carry two edits to the same file, and a read-modify-write
 * interleaved with another would silently drop one of them.
 */
export function createCapsuleTools(
  workspace: CapsuleWorkspace,
  validator: CapsuleValidator,
  hooks: CapsuleToolHooks = {},
): AgentTool<any>[] {
  const queue = createQueue();
  return [
    readTool(workspace),
    editTool(workspace, queue),
    writeTool(workspace, queue),
    validateTool(workspace, validator, queue, hooks),
    previewErrorsTool(hooks),
    ...(hooks.domains ? createDomainTools(hooks.domains) : []),
  ];
}

function readTool(workspace: CapsuleWorkspace): AgentTool<any> {
  return defineTool({
    name: "read",
    label: "Read file",
    description:
      "Read one existing Capsule file. Every current file was already included in the first message, " +
      "so read only to re-check a file after several edits, or to fetch a line range of a long file. " +
      "Use offset/limit to read part of a file; omit both to read all of it.",
    parameters: Type.Object({
      path: Type.String({ description: "Capsule file path, for example /workspace/app.js" }),
      offset: Type.Optional(Type.Integer({ minimum: 1, description: "First line to return, 1-based. Defaults to 1." })),
      limit: Type.Optional(Type.Integer({ minimum: 1, description: "Maximum number of lines to return." })),
    }),
    async execute(_toolCallId, params: { path: string; offset?: number; limit?: number }, signal?: AbortSignal) {
      const file = requireCapsuleFile(params.path);
      const content = unwrap(await workspace.env.readTextFile(capsulePathFor(file), signal));
      const lines = content.split("\n");
      const start = Math.max((params.offset ?? 1) - 1, 0);
      const end = params.limit === undefined ? lines.length : Math.min(start + params.limit, lines.length);
      const slice = lines.slice(start, end);
      const partial = start > 0 || end < lines.length;
      const header = partial ? `${file} lines ${start + 1}-${end} of ${lines.length}\n` : "";
      return result(header + slice.join("\n"), {
        path: capsulePathFor(file),
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
      "exactly once — add surrounding lines until it is unique. After editing /workspace/schema.sql or /workspace/actions.json " +
      "you must call validate_capsule.",
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

function writeTool(workspace: CapsuleWorkspace, queue: Queue): AgentTool<any> {
  return defineTool({
    name: "write",
    label: "Write file",
    executionMode: "sequential",
    description:
      "Replace an existing Capsule file or create a new one at any nested path inside /workspace. " +
      "Prefer edit unless you are genuinely rewriting the file, because a full rewrite is slow and easily drops unrelated details. " +
      "After writing /workspace/schema.sql or /workspace/actions.json you must call validate_capsule.",
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

/**
 * The Validation Gate proves the data layer holds; it cannot tell whether the
 * app still runs. This is the other half: what the live preview actually threw.
 */
function previewErrorsTool(hooks: CapsuleToolHooks): AgentTool<any> {
  return defineTool({
    name: "preview_errors",
    label: "Preview errors",
    description:
      "Read the runtime errors the live preview reported while running the current files — the failures the Validation Gate " +
      "cannot see, such as a missing element id that app.js queries, or a call that throws. " +
      "Call it after editing index.html or app.js to confirm the app still runs. " +
      "An empty report means the preview raised nothing since it last loaded, not that the app is correct.",
    parameters: Type.Object({}),
    async execute() {
      const failures = hooks.previewErrors?.() ?? [];
      if (failures.length === 0) {
        return result("No runtime errors reported by the preview since it last loaded.", { failures });
      }
      const lines = failures.map((failure) => {
        const repeats = failure.count > 1 ? ` (x${failure.count})` : "";
        const stack = failure.stack ? `\n${failure.stack.split("\n").slice(0, 4).join("\n")}` : "";
        return `- ${failure.kind}: ${failure.message}${repeats}${stack}`;
      });
      return result(`The preview reported ${failures.length} runtime failure(s):\n${lines.join("\n")}`, { failures });
    },
  });
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
      "Run the Validation Gate on the current workspace: schema.sql must execute — twice, because it is re-run on every database " +
      "open — on a fresh SQLite database and on a copy of the live one, actions.json must be a valid server action program, and " +
      "every action statement must prepare against that schema with its declared bindings. " +
      "Call it after every change to schema.sql or actions.json, and again after fixing reported errors. " +
      "The change cannot be applied to the Project until this passes.",
    parameters: Type.Object({}),
    async execute() {
      return queue(async () => {
        // The snapshot is taken inside the queue, so it is exactly what gets validated.
        const files = workspace.serialize().files;
        const validation = await validator.validate(files, await readLiveDatabase(hooks));
        hooks.onValidated?.(files, validation);
        return result(formatValidation(validation), validation);
      });
    },
  });
}

/**
 * What the gate gets to know about the live database. A read that fails is
 * reported as such rather than as "there is none": the migration stage is
 * skipped either way, but only one of the two is a stage nobody could check.
 */
export async function readLiveDatabase(hooks: CapsuleToolHooks): Promise<LiveDatabase> {
  if (!hooks.liveDatabase) return { state: "absent" };
  try {
    const bytes = await hooks.liveDatabase();
    return bytes ? { state: "loaded", bytes } : { state: "absent" };
  } catch (error) {
    return { state: "unreadable", reason: error instanceof Error ? error.message : String(error) };
  }
}

/** Widen a concretely-typed tool: the loop validates arguments against `parameters` before calling `execute`. */
function defineTool<T extends TSchema, D>(definition: AgentTool<T, D>): AgentTool<any> {
  return definition as unknown as AgentTool<any>;
}

export function formatValidation(validation: ValidationResult): string {
  const warnings = (validation.warnings ?? []).map((warning) => `\n- [${warning.stage}] Warning: ${warning.message}`).join("");
  if (validation.ok) {
    const skipped = validation.checked.includes("migration")
      ? ""
      : " The migration stage did not run, so nothing was checked against the live database.";
    return `Validation passed: schema, server actions and bindings are consistent.${skipped}${warnings}`;
  }
  const lines = validation.errors.map((error) => {
    const detail = error.detail ? `\n  ${error.detail}` : "";
    return `- [${error.stage}] ${error.message}${detail}`;
  });
  return `Validation failed with ${validation.errors.length} problem${validation.errors.length === 1 ? "" : "s"}:\n${lines.join("\n")}${warnings}`;
}

function requireCapsuleFile(path: string): ProjectFile {
  const file = capsuleFileFor(path);
  if (!file) throw new Error(`${path} is not a safe Capsule file path. Files must stay inside /workspace.`);
  return file;
}

function validationReminder(file: ProjectFile): string {
  return file === "schema.sql" || file === "actions.json" ? " Call validate_capsule now." : "";
}

type Snippet = { matches: number; index: number; needle: string };

/**
 * Locate `oldString` literally. A Capsule imported from a CRLF file cannot be
 * edited byte-for-byte — the model never sees the \r — so a snippet that misses
 * is retried in its CRLF form before the edit is refused.
 */
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

/** Overlapping occurrences count too: "must appear exactly once" has to mean it. */
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

/** One workspace, one queue: no read-modify-write may interleave with another. */
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
