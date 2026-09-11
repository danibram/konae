# pi API reference (`@earendil-works/pi-agent-core` + `@earendil-works/pi-ai` 0.84.2)

Extracted from `node_modules/@earendil-works/*/dist/**/*.d.ts` and the shipped `.js`. Local `.d.ts` is the only truth; this file is a build-time crib sheet, not a spec.

## 0. What is actually implemented in 0.84.2 (read this first)

`AgentHarness` is a **skeleton**. `AgentHarness.create()` succeeds on an empty session, but `prompt`, `abort`, `steer`, `followUp`, `compact`, `watch`, `watchSession`, `lane`, `lanes`, `resume`, `runToCompletion`, `waitForIdle` all reject with `HarnessNotImplemented`, and `harness.hooks.on` / `harness.events.on` throw. Verified in `dist/harness/agent-harness.js`.

**Use the `Agent` class** (`dist/agent.js`, fully implemented) — it owns the transcript, emits lifecycle events, executes tools, and supports abort/steer/follow-up. The harness types below are documented because the ADR names them, and because `AgentHarnessTool` is the shape the built-in tool factories return.

Imports: everything comes from `@earendil-works/pi-agent-core` (root; browser-clean) — never `@earendil-works/pi-agent-core/node`, which exports `NodeExecutionEnv` (`node:fs`). pi-ai: `@earendil-works/pi-ai` plus subpath `@earendil-works/pi-ai/providers/<id>`.

## 1. Session (in-memory)

`InMemorySessionRepo` and `InMemorySessionStorage` exist and are exported from the root entry (`dist/harness/session/memory.js`).

```ts
import { InMemorySessionRepo, Session, InMemorySessionStorage } from "@earendil-works/pi-agent-core";
const session = await new InMemorySessionRepo().create({});           // Promise<Session>
// or: new Session(new InMemorySessionStorage({ id: crypto.randomUUID(), createdAt: Date.now() }))
```

`SessionCreateOptions = { id?: string; parentSessionId?: string }`. `SessionMetadata = { id; createdAt; parentSessionId? }`.
`Session` implements `SessionTree`: `getLeafId()`, `getEntry(id)`, `getStats()`, `get/setName()`, `get/setLabel(id)`, `findEntries(q)`, `findEntry(q)`, `findEntriesOnBranch(q)`, `appendMessage(msg): Promise<string>`, `appendCustomEntry(type, data)`, plus lane/entry/record writes. `Entry` union: `message | model_change | thinking_level_change | active_tools_change | compaction | branch_summary | custom`. `JsonlSessionRepo` also exists and is browser-safe (it takes an injected `JsonlSessionRepoFileSystem`), but is unnecessary here.

`AgentHarnessOptions` (for reference): `{ session: Session; models: Models; model: Model<Api>; thinkingLevel?; activeToolNames?: string[]; tools?: HarnessTool[]; toolContext?: object | (() => object | Promise<object>); systemPrompt?: string | (() => string | Promise<string>); resources?; streamOptions?: SimpleStreamOptions; retry?: RetryPolicy; compaction?: CompactionSettings; steeringMode?; followUpMode?; toolExecution?: "sequential"|"parallel"; drive?: "automatic"|"manual"; toProviderMessages?; entryProjectors?; context?: TelemetryContext }`.

## 2. `Agent` — the usable loop

```ts
import { Agent, type AgentEvent, type AgentMessage } from "@earendil-works/pi-agent-core";
const agent = new Agent({ streamFn, initialState: { systemPrompt, model, thinkingLevel, tools }, ... });
```

`AgentOptions`: `streamFn` (**required**), `initialState?: Partial<{ systemPrompt: string; model: Model<any>; thinkingLevel: ThinkingLevel; tools: AgentTool[]; messages: AgentMessage[] }>`, `convertToLlm?`, `transformContext?`, `getApiKey?: (provider) => string|undefined|Promise<…>`, `onPayload?`, `onResponse?`, `beforeToolCall?`, `afterToolCall?`, `shouldStopAfterTurn?`, `prepareNextTurn?`, `prepareNextTurnWithContext?`, `steeringMode?`, `followUpMode?`, `sessionId?`, `thinkingBudgets?`, `transport?`, `maxRetryDelayMs?`, `toolExecution?`.

Methods: `prompt(text, images?) | prompt(msg | msg[]): Promise<void>`, `continue()`, `abort()`, `waitForIdle()`, `reset()`, `steer(msg)`, `followUp(msg)`, `clearAllQueues()`, `hasQueuedMessages()`, `subscribe(listener): () => void`, `get state(): AgentState`, `get signal(): AbortSignal | undefined`.
`AgentState` = `{ systemPrompt; model; thinkingLevel; tools; messages; isStreaming; streamingMessage?; pendingToolCalls: ReadonlySet<string>; errorMessage? }`. `state.systemPrompt`, `state.model`, `state.thinkingLevel`, `state.tools`, `state.messages` are writable (assigning arrays copies them).

`ThinkingLevel` (agent-core) = `"off"|"minimal"|"low"|"medium"|"high"|"xhigh"|"max"`.

`streamFn: (model: Model<Api>, context: Context, options?: SimpleStreamOptions) => AssistantMessageEventStream | Promise<…>`. It must not throw — encode failures as a final `AssistantMessage` with `stopReason: "error"|"aborted"`. `Models.streamSimple` satisfies it. **The agent forwards only `{...loopConfig, apiKey, signal}` to `streamFn`; `headers`, `cacheRetention`, `maxTokens`, `env`, `transformHeaders` are NOT forwarded — inject them inside your own `streamFn` wrapper.** `getApiKey(provider)` is resolved per turn (good for BYOK).

### Streaming events (`agent.subscribe((ev, signal) => …)`)

`AgentEvent` discriminated union — `agent_start` | `agent_end {messages}` | `turn_start` | `turn_end {message, toolResults}` | `message_start {message}` | `message_update {message, assistantMessageEvent}` | `message_end {message}` | `tool_execution_start {toolCallId, toolName, args}` | `tool_execution_update {…, partialResult}` | `tool_execution_end {toolCallId, toolName, result, isError}`.

- **assistant text deltas**: `message_update` → `ev.assistantMessageEvent` is an `AssistantMessageEvent`: `start | text_start | text_delta {contentIndex, delta} | text_end | thinking_start | thinking_delta | thinking_end | toolcall_start | toolcall_delta | toolcall_end {toolCall} | done {reason, message} | error {reason, error}`.
- **tool start/end**: `tool_execution_start` / `tool_execution_end` (`isError` flag; `result` is the `AgentToolResult`).
- **turn end**: `turn_end`; **run end**: `agent_end` (last event; the agent is idle only after awaited `agent_end` listeners settle).
- Listener promises are awaited in subscription order and receive the run's `AbortSignal`.
- **abort**: `agent.abort()` aborts the active run; there is no per-message abort. `await agent.waitForIdle()` to join.
- errors: a failed turn sets `state.errorMessage` and ends with an assistant message whose `stopReason` is `"error"`/`"aborted"` (surfaced through `message_end` / `turn_end`), not by rejecting `prompt()`.

## 3. Tools

`Tool<TParameters extends TSchema>` (pi-ai) = `{ name; description; parameters: TParameters; constrainedSampling? }`. Schemas are **TypeBox** (`typebox@1.3.7`); re-exported as `import { Type, type Static, type TSchema } from "@earendil-works/pi-ai"`.

```ts
interface AgentTool<TParameters extends TSchema = TSchema, TDetails = any> extends Tool<TParameters> {
  label: string;
  prepareArguments?: (args: unknown) => Static<TParameters>;
  execute(toolCallId: string, params: Static<TParameters>, signal?: AbortSignal,
          onUpdate?: (partial: AgentToolResult<TDetails>) => void): Promise<AgentToolResult<TDetails>>;
  executionMode?: "sequential" | "parallel";
}
interface AgentToolResult<T> { content: (TextContent|ImageContent)[]; details: T; usage?: Usage;
                               addedToolNames?: string[]; terminate?: boolean }
```

`execute` **throws** on failure (the loop converts the throw into an `isError: true` tool result); it must not encode errors in `content`.

`AgentHarnessTool<TContext, TParameters, TDetails>` is `AgentTool` with a 5th `context: TContext` argument on `execute`. The built-in factories return this shape, so bind the context to use them with `Agent`:

```ts
const bind = <C extends object>(t: AgentHarnessTool<C>, ctx: C): AgentTool =>
  ({ ...t, execute: (id, p, signal, onUpdate) => t.execute(id, p, signal, onUpdate, ctx) });
```

Built-ins (`ExecutionToolContext = { env: ExecutionEnv }`): `createEditTool()` — `{ path: string; edits: { oldText; newText }[] }`, targeted unique non-overlapping replacements, details `{ diff; patch; firstChangedLine? }`; `createWriteTool()` — `{ path; content }`; `createReadTool(options?)` — `{ path; offset?; limit? }`; `createBashTool(options)` — **do not use** (no `Shell` in the browser). All resolve paths through `env.absolutePath` and serialize mutations per canonical path via `withFileMutationQueue`. `edit` requires `fileInfo` to report `kind: "file"|"symlink"` and `readTextFile` to succeed.

## 4. `ExecutionEnv = FileSystem & Shell`

`Result<V, E> = { ok: true; value: V } | { ok: false; error: E }`; helpers `ok`, `err`, `getOrThrow`, `getOrUndefined`, `toError`.
`FileError` (`code: "aborted"|"not_found"|"permission_denied"|"not_directory"|"is_directory"|"invalid"|"not_supported"|"unknown"`, `path?`). `ExecutionError` (`code: "aborted"|"timeout"|"shell_unavailable"|"spawn_error"|"callback_error"|"unknown"`).
`FileInfo = { name; path; kind: "file"|"directory"|"symlink"; size; mtimeMs }`.
**No method may throw or reject** — every failure is an `ok: false` Result. `cleanup()` is best-effort and must not throw.

```ts
interface FileSystem {
  cwd: string;
  absolutePath(path, abortSignal?): Promise<Result<string, FileError>>;
  joinPath(parts: string[], abortSignal?): Promise<Result<string, FileError>>;
  readTextFile(path, abortSignal?): Promise<Result<string, FileError>>;
  readTextLines(path, options?: { maxLines?: number; abortSignal?: AbortSignal }): Promise<Result<string[], FileError>>;
  readBinaryFile(path, abortSignal?): Promise<Result<Uint8Array, FileError>>;
  writeFile(path, content: string | Uint8Array, abortSignal?): Promise<Result<void, FileError>>;
  appendFile(path, content: string | Uint8Array, abortSignal?): Promise<Result<void, FileError>>;
  renameFile(sourcePath, destinationPath, abortSignal?): Promise<Result<void, FileError>>;
  fileInfo(path, abortSignal?): Promise<Result<FileInfo, FileError>>;
  listDir(path, abortSignal?): Promise<Result<FileInfo[], FileError>>;
  canonicalPath(path, abortSignal?): Promise<Result<string, FileError>>;
  exists(path, abortSignal?): Promise<Result<boolean, FileError>>;   // false for missing; FileError for other failures
  createDir(path, options?: { recursive?: boolean; abortSignal?: AbortSignal }): Promise<Result<void, FileError>>;
  remove(path, options?: { recursive?: boolean; force?: boolean; abortSignal?: AbortSignal }): Promise<Result<void, FileError>>;
  createTempDir(prefix?, abortSignal?): Promise<Result<string, FileError>>;
  createTempFile(options?: { prefix?: string; suffix?: string; abortSignal?: AbortSignal }): Promise<Result<string, FileError>>;
  cleanup(): Promise<void>;
}
interface Shell {
  exec(command: string, options?: ShellExecOptions):
    Promise<Result<{ stdout: string; stderr: string; exitCode: number }, ExecutionError>>;
  cleanup(): Promise<void>;
}
// ShellExecOptions = { cwd?; env?; inheritEnv?; timeout? /* seconds */; abortSignal?; onStdout?; onStderr? }
```

For the Capsule: implement every `FileSystem` method over the five-file record; `exec` returns `err(new ExecutionError("shell_unavailable", …))`.

## 5. pi-ai: models and providers

```ts
import { createModels, type Model, type Api, type Models } from "@earendil-works/pi-ai";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";     // Provider<"anthropic-messages">
import { openaiProvider } from "@earendil-works/pi-ai/providers/openai";           // Provider<"openai-responses">
import { openrouterProvider } from "@earendil-works/pi-ai/providers/openrouter";   // Provider<"openai-completions">

const models = createModels();                     // MutableModels; defaults: InMemoryCredentialStore,
models.setProvider(anthropicProvider());           //   InMemoryModelsStore, defaultProviderAuthContext()
const model = models.getModel("anthropic", "claude-sonnet-4-5"); // Model<Api> | undefined (sync catalog read)
```

Register only the three providers you need — do **not** import `@earendil-works/pi-ai/providers/all` (pulls the whole generated catalog and the Bedrock provider). Each factory carries a static catalog: anthropic 13 ids (`claude-opus-5`, `claude-sonnet-5`, `claude-sonnet-4-6`, `claude-opus-4-8`, `claude-haiku-4-5`, `claude-fable-5`, …), openai 38 (`gpt-5.6-*`, `gpt-5.4`, `gpt-5-mini`, `o3`, …), openrouter 346 (`anthropic/claude-opus-4.8`, `openai/…`, prefixed ids; its catalog JSON is ~140 kB — keep the agent chunk lazy).

**Requests**: `models.streamSimple(model, context, options)` → `AssistantMessageEventStream` (sync return, async setup behind it; `.result()` resolves the final `AssistantMessage`). `Context = { systemPrompt?; messages: Message[]; tools?: Tool[] }`.

**baseURL**: the request URL is `model.baseUrl`. Override per request by cloning the model: `{ ...model, baseUrl: "http://localhost:1234/v1" }`. `AuthResult.auth.baseUrl` also wins over `model.baseUrl` (`applyAuth` does `{...model, baseUrl: auth.baseUrl}`). `Provider.baseUrl` is metadata for new provider definitions (`createProvider`).

**Keys and extra headers** — there is **no `getApiKeyAndHeaders`** in 0.84.2. `ModelsSimpleStreamOptions = SimpleStreamOptions & { transformHeaders?: (h: ProviderHeaders) => ProviderHeaders | Promise<…> }`. In `Models.applyAuth`: passing `options.apiKey` short-circuits credential/env resolution entirely (the BYOK path); `options.headers` (`Record<string, string | null>`, `null` deletes a default) is merged over resolved auth headers per header name case-insensitively; `transformHeaders` runs last on the merged set. Alternatively store a key with `credentialStore.modify(id, async () => ({ type: "api_key", key }))`, or resolve/inspect with `models.getAuth(model | providerId, overrides?)` → `{ auth: { apiKey?, headers?, baseUrl? }, env?, source? }`. `Model.headers` is a static per-model header bag.

**Prompt caching** is on by default and needs no flag: the Anthropic adapter writes `cache_control: { type: "ephemeral" }` onto the system prompt, the last tool definition, and the last user/assistant/tool-result text block. `SimpleStreamOptions.cacheRetention?: "none" | "short" | "long"` (default `"short"` ≈ 5 min; `"long"` adds `ttl: "1h"` when `compat.supportsLongCacheRetention`). For OpenRouter, caching is driven by `model.compat.cacheControlFormat: "anthropic"` — already set in the shipped catalog for `anthropic/*` models. `Usage` reports `cacheRead`, `cacheWrite`, `cacheWrite1h`.

Other `SimpleStreamOptions` worth knowing: `reasoning?: "minimal"|"low"|"medium"|"high"|"xhigh"|"max"`, `thinkingBudgets?`, `maxTokens?`, `temperature?`, `metadata?`, `sessionId?`, `signal?`, `timeoutMs?`, `maxRetries?`, `maxRetryDelayMs?`, `transport?: "sse"|"websocket"|"websocket-cached"|"auto"`, `fetch?`, `env?`, `onPayload?`, `onResponse?`. Thinking support per model: `getSupportedThinkingLevels(model)` / `clampThinkingLevel(model, level)`.

## 6. Browser hazards

- **Never import** `@earendil-works/pi-agent-core/node` (`NodeExecutionEnv` → `node:fs`), `@earendil-works/pi-ai/bedrock-provider`, `@earendil-works/pi-ai/bun-oauth`, `@earendil-works/pi-ai/dist/cli.js`, or the `amazon-bedrock` / `openai-codex` providers — those API adapters import `utils/node-http-proxy.js` (`node:http`) and the AWS SDK.
- Do not call `Provider.auth.oauth.login()` in the browser: OAuth flows are loaded through a bundler-opaque dynamic import and use `node:http`/`node:crypto` callback servers. BYOK api-key auth only.
- `defaultProviderAuthContext()` is browser-safe: `env()` reads `globalThis.process?.env` (undefined → no key), `fileExists()` always false via a variable-specifier dynamic import.
- `anthropic-dangerous-direct-browser-access: "true"` and `dangerouslyAllowBrowser: true` are **already set** by the Anthropic adapter (and `dangerouslyAllowBrowser` by both OpenAI adapters) — no extra header is needed for direct browser calls. Anthropic still rejects requests with no key: `assertRequestAuth` throws unless `apiKey` or an `authorization`/`x-api-key` header is present.
- `utils/provider-env.js` contains a `require("node:fs")` inside a Bun-sandbox guard (`process.versions.bun` + empty `process.env`); unreachable in a browser and it bundles fine, but do not "fix" it by shimming `require`.
- No shell: never register `createBashTool`.
- Verified: `vite build` (Vite 6.4 / Rollup 4) bundles `Agent`, `InMemorySessionRepo`, the three provider factories and the file tools cleanly. JSON import attributes (`with { type: "json" }`) in the catalogs resolve. The provider API adapters are behind `lazyApi()` dynamic imports, so `anthropic-messages` (~186 kB) / `openai-completions` (~32 kB) / `openai-responses` (~26 kB) land in separate chunks loaded on first request.
