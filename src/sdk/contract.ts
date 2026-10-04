/**
 * The miniapp SDK contract: every module on `window.miniapp`, every call, its
 * arguments, what it returns, what it refuses and the limits that apply.
 *
 * This file is the single source. `bun run sdk:render` writes three readers
 * from it: the SDK section of the build agent's prompt
 * (src/agent/sdk-section.generated.ts), the `sdk` topic of `platform_guide`
 * (kernel-rs/src/sdk-guide.json) and docs/sdk.md. src/sdk/contract.test.ts
 * fails when src/mcp/runtime.js, the preview client or any reader disagrees.
 *
 * Error messages are the exact text the published app sees: the Kernel's,
 * or the SDK's own when `source` is "sdk". `preview` records where the
 * editor preview differs: absent means the same text, null means the preview
 * never refuses that case, a list holds the preview's own messages for it.
 */

export type SdkArgument = {
  name: string;
  type: string;
  required: boolean;
  description: string;
};

export type SdkError = {
  message: string;
  when: string;
  source: "kernel" | "sdk";
  /** The HTTP status a route answers with, for refusals seen on an HTTP route. */
  status?: number;
  /** Exact messages; placeholders are written <key> and <name>. */
  preview?: readonly string[] | null;
};

export type SdkExample = {
  code: string;
  comment?: string;
};

export type SdkCall = {
  /** The property on the module, e.g. "call" for miniapp.actions.call. */
  name: string;
  signature: string;
  summary: string;
  /** Whether a visitor without a session may use it on a public Deployment. */
  signedOut: boolean;
  /** The runtime MCP tools the published SDK calls, in kernel-rs/src/mcp.rs runtime_tools(). */
  tools: string[];
  args: SdkArgument[];
  returns: string;
  errors: SdkError[];
  limits: string[];
  examples: SdkExample[];
  /** How the editor preview behaves differently, when it does. */
  preview?: string;
};

/**
 * A key a module adds to a Capsule file, for modules that are declared rather
 * than called. `errors` lists what publishing or the declared route refuses.
 */
export type SdkDeclaration = {
  file: string;
  key: string;
  summary: string;
  errors?: SdkError[];
  limits?: string[];
};

export type SdkModule = {
  name: string;
  summary: string;
  calls: SdkCall[];
  declarations: SdkDeclaration[];
  notes: string[];
};

export type SdkContract = {
  version: 1;
  global: "miniapp";
  endpoint: string;
  summary: string;
  /** Things an app may not rely on, stated so neither agent invents them. */
  absent: string[];
  /** Refusals any call that reaches the Kernel can return. */
  errors: SdkError[];
  modules: SdkModule[];
};

const download: SdkError[] = [
  { message: "File not found.", when: "the file was removed or the actor may not read it", source: "kernel", preview: null },
  { message: "Too many active transfers. Cancel completed transfers before starting another.", when: "the actor already holds 2 transfers, or the Deployment holds 32", source: "kernel", preview: null },
  { message: "Transfer not found or expired.", when: "a transfer outlived its 10 minutes", source: "kernel", preview: null },
  { message: "Invalid file size.", when: "the Kernel announced a size outside 0 to 2 MiB", source: "sdk", preview: null },
  { message: "Invalid file block.", when: "a downloaded block does not continue at the expected offset or exceeds 64 KiB", source: "sdk", preview: null },
  { message: "File checksum mismatch.", when: "the downloaded bytes do not match the SHA-256 the Kernel announced", source: "sdk", preview: null },
];

const noFileId: SdkError = { message: "A file ID is required.", when: "fileId is not a non-empty string", source: "sdk" };
const badVisibility: SdkError = { message: "File visibility must be private or public.", when: "visibility is anything other than \"private\" or \"public\"; the SDK refuses before it sends anything", source: "sdk" };
const notUploader: SdkError = { message: "Only the person who uploaded this file may change or delete it.", when: "the caller may read the file but did not upload it", source: "kernel" };
const unreadable: SdkError = { message: "File not found.", when: "the file does not exist, was deleted, or the caller may not read it", source: "kernel" };

const fileShape = "{ id, name, type, size, visibility, createdAt, ownerId, url }. ownerId is present only on files the caller uploaded. url is a Blob URL; it keeps the type for image/png, image/jpeg, image/gif, image/webp and image/avif and is application/octet-stream otherwise";

export const sdkContract = {
  version: 1,
  global: "miniapp",
  endpoint: "/_littlebox/mcp",
  summary: "window.miniapp is the only host surface a Capsule has, in the editor preview and on every Deployment. Each module is a namespace on it. A published call is one or more MCP tool calls to the runtime endpoint on the host the page loaded from, the Deployment's generated hostname or its custom domain, with that host's app session cookie.",
  absent: [
    "no fetch to other origins",
    "no localStorage contract",
    "no miniapp.db and no SQL from the browser",
    "no miniapp.track and no analytics call: a Signal is declared on a Server Action and recorded when miniapp.actions.call runs it",
  ],
  errors: [
    { message: "Sign in to use Littlebox MCP.", when: "the Deployment is private and the visitor has no session; actions.call then sends the browser to /_littlebox/login and auth.session returns the anonymous state instead", source: "kernel", preview: null },
    { message: "Deployment not found.", when: "the host name matches no Deployment, or is a custom domain whose TXT record domain_set has not found yet", source: "kernel", preview: null },
    { message: "Cross-origin or unrecognized-host MCP request rejected.", when: "the request does not come from the origin of the host it is sent to, the Deployment's generated hostname or its verified custom domain", source: "kernel", preview: null },
    { message: "MCP request exceeds 200 KB.", when: "one request body is larger than 200,000 bytes", source: "kernel", preview: null },
    { message: "Invalid MCP response.", when: "the response does not answer the request the SDK sent", source: "sdk", preview: null },
  ],
  modules: [
    {
      name: "auth",
      summary: "Who is using the app. The Kernel owns accounts and sessions; the app only reads the current one.",
      declarations: [],
      notes: [
        "Visibility decides who may open the app shell. It never grants data access on its own.",
        "The Deployment's Signup Policy, which the owner sets in the editor or with signup_policy_set, decides who may create an account: Open, Invitation only, Closed or an Allowlist. Under Invitation only, the sign-in page offers no account creation; invited people accept from the link in their email, which opens /_littlebox/invitation.",
      ],
      calls: [
        {
          name: "session",
          signature: "miniapp.auth.session()",
          summary: "Read the signed-in Runtime Actor, or the anonymous state so the app can show a sign-in button.",
          signedOut: true,
          tools: ["auth_session"],
          args: [],
          returns: "{ userId, name, role }. Signed in, name is the account email and role is \"member\" or \"owner\". Signed out, { userId: \"anonymous\", name: \"Anonymous\", role: \"anonymous\" }",
          errors: [],
          limits: [],
          examples: [{ code: "const session = await miniapp.auth.session();", comment: "{ userId, name, role }; role is \"anonymous\" when signed out" }],
          preview: "Returns Alice or Bob, the preview's test accounts, with role \"member\", or the anonymous state.",
        },
        {
          name: "signIn",
          signature: "miniapp.auth.signIn()",
          summary: "Send the visitor to the Deployment's login page at /_littlebox/login.",
          signedOut: true,
          tools: [],
          args: [],
          returns: "nothing; the page navigates away",
          errors: [],
          limits: [],
          examples: [{ code: "await miniapp.auth.signIn();", comment: "published login page, preview selects Alice" }],
          preview: "Signs in as Alice without leaving the page and returns her session.",
        },
        {
          name: "signOut",
          signature: "miniapp.auth.signOut()",
          summary: "Revoke this app session, clear its cookie and release the file URLs the SDK created.",
          signedOut: false,
          tools: ["auth_logout"],
          args: [],
          returns: "{ ok: true }",
          errors: [
            { message: "authentication-required", when: "no one is signed in", source: "kernel", preview: null },
          ],
          limits: [],
          examples: [{ code: "await miniapp.auth.signOut();", comment: "revoke this app session" }],
          preview: "Switches to the signed-out state and returns the anonymous session.",
        },
      ],
    },
    {
      name: "actions",
      summary: "The only door to the database. A call names a Server Action declared in actions.json and passes its input; the Kernel authorizes the caller, binds the input and runs the action's one SQL statement, or records its Signal.",
      declarations: [
        {
          file: "actions.json",
          key: "signal",
          summary: "{ \"name\": \"order.placed\", \"properties\"?: [\"input binding keys\", \"actor\"] } on a mutation, or on an action of \"kind\": \"signal\" that has access and bindings but no statement. When the action succeeds, and for a mutation only when it changed at least one row, the Kernel records the Signal with the named inputs as its properties, in the Deployment's Usage store. The property actor records the caller's ID: the user's, the API key's, or null for a visitor or a signed request. A signal action runs no SQL and returns { recorded: true }. The owner reads Signals with signals_read; the app cannot. The Validation Gate and the Kernel refuse the same declarations with the same messages.",
          errors: [
            { message: "Action <name> is a query, so it cannot declare a signal. Declare it on a mutation or on a \"kind\": \"signal\" action.", when: "the action is a query", source: "kernel" },
            { message: "Action <name> names signal property <key>, which is not one of its input bindings.", when: "a property is not the key of an input binding; actor, kernel and literal bindings have no key", source: "kernel" },
            { message: "Action <name> names signal property <key> twice.", when: "properties lists a key twice", source: "kernel" },
            { message: "Action <name> names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.", when: "properties lists actor and an input binding's key is actor", source: "kernel" },
            { message: "Action <name> names more than 8 signal properties.", when: "properties lists more than 8 keys", source: "kernel" },
            { message: "Action <name> names an invalid signal. Use dot-separated lowercase words, such as \"order.placed\", up to 64 characters.", when: "the name has one word, a capital, a space, an empty part or more than 64 characters", source: "kernel" },
            { message: "Action <name> has an invalid signal declaration.", when: "signal is not an object, has a key other than name and properties, has no name, or properties is not a list of strings", source: "kernel" },
            { message: "Action <name> is a signal action, so it must declare a signal.", when: "the kind is signal and there is no signal key", source: "kernel" },
            { message: "Action <name> is a signal action, so it runs no SQL and cannot declare a statement.", when: "the kind is signal and the action has a statement", source: "kernel" },
          ],
          limits: [
            "8 properties per Signal; a string property keeps its first 128 bytes",
            "a signal action counts toward the action limit like any other action",
            "a signal action cannot declare http",
            "a mutation that changed no row records no Signal",
            "the Kernel keeps the newest 50,000 Signals per Deployment and drops the oldest",
          ],
        },
        {
          file: "actions.json",
          key: "guard",
          summary: "\"guard\": \"<message>\" on a mutation. When its statement changes no row, the Kernel refuses the call with that message instead of answering { changes: 0, lastInsertId: 0 }: a tool error for miniapp.actions.call, 403 on an HTTP route. Usage counts the call under refused, and no Signal is recorded. Write the app's rule into the statement's WHERE, such as \"the caller is an organizer\" or \"the slot is free\", and the guard is what the person sees when the rule says no. Without a guard a mutation answers { changes: 0 } as before. The Validation Gate and the Kernel refuse the same declarations with the same messages.",
          errors: [
            { message: "Action <name> is a query, so it cannot declare a guard. Declare it on a mutation.", when: "the action is a query", source: "kernel" },
            { message: "Action <name> is a signal action, so it cannot declare a guard. Declare it on a mutation.", when: "the action is a signal action", source: "kernel" },
            { message: "Action <name> has an invalid guard. Use a message of 1 to 200 characters on one line.", when: "guard is not a string, is empty or blank, is longer than 200 characters, or holds a line break or another control character", source: "kernel" },
          ],
          limits: [
            "a guard is 1 to 200 characters on one line",
          ],
        },
      ],
      notes: [
        "A query action returns the rows of its SELECT. Column names are exactly the ones the SELECT projects, so alias any expression the app reads.",
        "A mutation action returns { changes, lastInsertId }. Read changed rows with a separate query. A mutation that declares a guard and changes no row is refused with the guard instead.",
        "The Action Contract, action-contract.json, lists every action with its inputs, the columns it returns, its errors and limits. It is written from actions.json and schema.sql by littlebox contract and carried in the export; an expression the SELECT returns without AS has no name in it, and the Validation Gate warns.",
        "To record that something happened, such as a signup to a newsletter or a finished lesson, declare a signal on the mutation that does it, or add an action of kind signal and call it with miniapp.actions.call. There is no other tracking call.",
        "A call the Kernel refuses records no Signal and counts in no Usage total, except a guard's refusal, which Usage counts under refused.",
      ],
      calls: [
        {
          name: "call",
          signature: "miniapp.actions.call(name, input = {})",
          summary: "Invoke one declared Server Action as the current Runtime Actor.",
          signedOut: true,
          tools: ["actions_invoke"],
          args: [
            { name: "name", type: "string", required: true, description: "A Server Action name declared in actions.json, e.g. tasks.list" },
            { name: "input", type: "object", required: false, description: "One value per input binding of the action, keyed by the binding's key. Defaults to {}" },
          ],
          returns: "an array of row objects for a query action, { changes, lastInsertId } for a mutation action, { recorded: true } for a signal action",
          errors: [
            { message: "Server action not declared.", when: "actions.json declares no action with that name", source: "kernel" },
            { message: "authentication-required", when: "the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login", source: "kernel" },
            { message: "rate-limited", when: "more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user", source: "kernel", preview: null },
            { message: "Arguments must be an object.", when: "input is not an object, e.g. null", source: "kernel" },
            { message: "Expected a string.", when: "name is not a string", source: "kernel" },
            { message: "Unexpected action input: <key>", when: "input has a key no input binding declares", source: "kernel" },
            { message: "This action requires an authenticated actor.", when: "an anonymous action binds the actor id and no one is signed in", source: "kernel" },
            { message: "Action <name> answers only on its HTTP route, <key>.", when: "the action's route does not answer anyone: it declares a signature or callers, or it has no signature on a private Deployment. The preview refuses the first two only, since it has no visibility", source: "kernel" },
            { message: "Missing action input: <key>", when: "a required input is omitted", source: "kernel" },
            { message: "<key> must be a non-empty string.", when: "a string input is not a string, or is empty or only whitespace", source: "kernel" },
            { message: "<key> is too long.", when: "a string input is longer than its maxLength in UTF-16 code units", source: "kernel" },
            { message: "<key> must be a number.", when: "a number input is not a JSON number", source: "kernel" },
            { message: "<key> must be a boolean.", when: "a boolean input is not true or false", source: "kernel" },
            { message: "Query result exceeds 2 MiB or 1000 rows.", when: "a query produces more than 1000 rows or more than 2 MiB of JSON", source: "kernel" },
          ],
          limits: [
            "30 calls per 10 seconds per signed-out client address and Deployment; 120 per 10 seconds per signed-in user",
            "1000 rows and 2 MiB per query result",
            "2 seconds per statement; SQLite's own error text is returned when a statement fails or runs out of time",
            "string inputs default to a maxLength of 10,000 UTF-16 code units",
            "256 MiB per app database",
          ],
          examples: [
            { code: "const rows = await miniapp.actions.call(\"tasks.list\");", comment: "array of row objects" },
            { code: "await miniapp.actions.call(\"tasks.create\", { title });", comment: "{ changes, lastInsertId }" },
            { code: "await miniapp.actions.call(\"lesson.finish\", { lesson });", comment: "a signal action: { recorded: true }" },
          ],
          preview: "Ignores the Deployment's visibility and rate limits, and keeps its database in the browser with a 10 MiB cap. A signal action returns { recorded: true } and records nothing; Signals exist only on a Deployment or a Development Kernel.",
        },
      ],
    },
    {
      name: "files",
      summary: "Small files stored by the Kernel for one Deployment. A file is private to the user who uploaded it unless the upload asked for public.",
      declarations: [],
      notes: [
        "Only the uploader can list or read a private file.",
        "On a public Deployment a signed-out visitor can list and read public files. On a private Deployment any signed-in user can.",
        "files.list and files.upload download over MCP, check the SHA-256 and return a Blob URL. The page's policy allows connect-src 'self' only, so fetch cannot read a Blob URL again. To fetch a file, or to show one without downloading every file, use files.url. It returns the file's path GET /_littlebox/files/<id> on the app's own origin, which img, fetch and a link all reach.",
        "The uploader alone changes a file's visibility with files.update or deletes it with files.delete. A deleted file stops counting against the 20 MiB quota and its ID answers File not found from then on, through every door.",
        "A visitor without a session on a private Deployment is asked to sign in before any file is looked up: 401 authentication-required. On a public Deployment, a file the visitor may not read answers 404 File not found, the same as a missing one. A signed-in user who may not read a file also gets 404.",
      ],
      calls: [
        {
          name: "list",
          signature: "miniapp.files.list()",
          summary: "List and download every file the current actor may read.",
          signedOut: true,
          tools: ["files_list", "files_read", "transfer_read", "transfer_cancel"],
          args: [],
          returns: `an array of ${fileShape}`,
          errors: download,
          limits: [
            "every listed file is downloaded, one at a time, on each call",
            "2 MiB per file",
          ],
          examples: [{ code: "const files = await miniapp.files.list();", comment: "that user's private files, plus public files" }],
          preview: "Reads files from browser storage and returns data URLs.",
        },
        {
          name: "upload",
          signature: "miniapp.files.upload(file, options)",
          summary: "Store a File or Blob for the signed-in user and return it as files.list would.",
          signedOut: false,
          tools: ["transfer_begin", "transfer_write", "transfer_complete", "files_upload", "transfer_cancel", "files_read", "transfer_read"],
          args: [
            { name: "file", type: "File | Blob", required: true, description: "The bytes to store, with its name and type" },
            { name: "options.visibility", type: "\"private\" | \"public\"", required: false, description: "\"private\", the default, keeps the file to the uploader. Pass \"public\" only when every visitor of a public app may read the bytes. Any other value is refused before the upload starts" },
          ],
          returns: fileShape,
          errors: [
            { message: "authentication-required", when: "no one is signed in", source: "kernel" },
            badVisibility,
            { message: "Transfer size is outside the limit.", when: "the file is empty or larger than 2 MiB", source: "kernel" },
            { message: "This deployment has reached its 20 MiB file quota.", when: "the Deployment's files would exceed 20 MiB together", source: "kernel", preview: null },
            ...download,
          ],
          limits: [
            "1 byte to 2 MiB per file",
            "20 MiB of files per Deployment",
            "2 transfers at a time per user and 32 per Deployment; each expires after 10 minutes",
            "the name is cut to 180 characters with / and \\ replaced by -; a type of 128 characters or more, or with a character other than a letter, a digit or one of !#$&^_.+-/, becomes application/octet-stream",
          ],
          examples: [
            { code: "const stored = await miniapp.files.upload(file);", comment: "private to the signed-in user" },
            { code: "const shared = await miniapp.files.upload(file, { visibility: \"public\" });" },
          ],
          preview: "Stores the file in browser storage as a data URL.",
        },
        {
          name: "url",
          signature: "miniapp.files.url(fileId)",
          summary: "The address of a stored file on the app's own origin, for an img src, a link or a fetch.",
          signedOut: true,
          tools: [],
          args: [{ name: "fileId", type: "string", required: true, description: "The id of a file from files.list or files.upload, or one an action returned" }],
          returns: "a Promise of the path \"/_littlebox/files/<id>\". Reading it answers the file's bytes with its own type when the caller may read it, 404 File not found when the file is missing, deleted or not theirs, and 401 authentication-required to a visitor of a private Deployment",
          errors: [
            noFileId,
            { ...unreadable, when: "reading the path, when the file does not exist, was deleted, or the caller may not read it (404)", status: 404 },
            { message: "authentication-required", when: "reading the path on a private Deployment without a session (401)", source: "kernel", status: 401, preview: null },
          ],
          limits: [
            "the path answers with Cache-Control: private, no-store and Cross-Origin-Resource-Policy: same-origin, so another site cannot embed it",
            "PNG, JPEG, GIF, WebP and AVIF answer inline; every other type downloads as an attachment",
          ],
          examples: [
            { code: "image.src = await miniapp.files.url(photo.file_id);", comment: "\"/_littlebox/files/<id>\"" },
            { code: "const text = await (await fetch(await miniapp.files.url(note.id))).text();" },
          ],
          preview: "Returns the file's data URL from browser storage, and refuses at the call with File not found when the preview actor may not read the file.",
        },
        {
          name: "update",
          signature: "miniapp.files.update(fileId, { visibility })",
          summary: "Make a file the signed-in user uploaded public or private. Publishing a draft keeps its ID, so no second copy counts against the quota.",
          signedOut: false,
          tools: ["files_update"],
          args: [
            { name: "fileId", type: "string", required: true, description: "The id of a file the signed-in user uploaded" },
            { name: "visibility", type: "\"private\" | \"public\"", required: true, description: "\"public\" lets every signed-in user, and every visitor of a public app, read the file; \"private\" keeps it to the uploader" },
          ],
          returns: "{ id, name, type, size, visibility, createdAt, ownerId } with the new visibility. There is no url; the ID and its files.url path stay the same",
          errors: [
            { message: "authentication-required", when: "no one is signed in", source: "kernel" },
            noFileId,
            badVisibility,
            unreadable,
            notUploader,
          ],
          limits: [],
          examples: [{ code: "await miniapp.files.update(draft.id, { visibility: \"public\" });", comment: "the draft is now published under the same ID" }],
        },
        {
          name: "delete",
          signature: "miniapp.files.delete(fileId)",
          summary: "Delete a file the signed-in user uploaded. Its bytes are gone and its size stops counting against the Deployment's quota.",
          signedOut: false,
          tools: ["files_delete"],
          args: [{ name: "fileId", type: "string", required: true, description: "The id of a file the signed-in user uploaded" }],
          returns: "{ id, deleted: true }",
          errors: [
            { message: "authentication-required", when: "no one is signed in", source: "kernel" },
            noFileId,
            { ...unreadable, when: "the file does not exist, was already deleted, or the caller may not read it" },
            notUploader,
          ],
          limits: ["a row that still names the file ID keeps it; the app removes that row with its own action"],
          examples: [{ code: "await miniapp.files.delete(note.file_id);", comment: "{ id, deleted: true }" }],
        },
      ],
    },
    {
      name: "http",
      summary: "Routes on the Deployment's own host for programs and other services: a JSON endpoint, a webhook, a file a program fetches. There is no browser call; the app's own pages keep using miniapp.actions.call, and may use a file's route as an img src.",
      calls: [],
      declarations: [
        {
          file: "actions.json",
          key: "http",
          summary: "{ \"method\": \"GET\" | \"POST\", \"path\": \"/api/items\", \"signature\"?: { \"header\": \"X-Signature\", \"secret\": \"items_webhook\" }, \"callers\"?: [\"signature\" | \"key\" | \"session\"] } on an action declared anonymous. GET binds a query and maps the query string onto its input bindings by type; POST binds a mutation and reads a JSON object body. callers names who may call the route; without it a signed route takes the signature or an API key, an unsigned route on a private Deployment an API key or a session, and an unsigned route on a public Deployment anyone. An action whose route does not answer anyone answers only on that route, never through miniapp.actions.call. The route answers { result } with the rows or { changes, lastInsertId }, and { error } with a status code. The Validation Gate and the Kernel refuse the same declarations with the same messages.",
          errors: [
            { message: "Action <name> must bind GET for a query and POST for a mutation.", when: "the method does not match the action's kind, or is neither GET nor POST", source: "kernel" },
            { message: "Action <name> is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.", when: "the action's access is authenticated", source: "kernel" },
            { message: "Action <name> has an invalid http path. Use a leading \"/\" and segments of letters, digits, \".\", \"_\", \"~\" or \"-\", up to 200 characters.", when: "the path is \"/\", ends with \"/\", climbs with \"..\", or carries a query string", source: "kernel" },
            { message: "Action <name> binds the reserved path <key>. Paths under /_littlebox, /oauth, /.well-known and /mcp belong to the Kernel.", when: "the first segment is reserved, in any letter case", source: "kernel" },
            { message: "Actions <name> and <name> both bind <key>.", when: "two actions declare the same method and path", source: "kernel" },
            { message: "Action <name> has an invalid signature header.", when: "the header is not 1 to 64 letters, digits and -, or is Authorization, Cookie, Host, Content-Type or Content-Length", source: "kernel" },
            { message: "Action <name> names an invalid signature secret. Use lowercase letters, digits and \"_\", starting with a letter.", when: "the secret is not a secret name", source: "kernel" },
            { message: "Action <name> has an invalid http binding.", when: "http is not an object, has another key, or its signature has a key other than header and secret", source: "kernel" },
            { message: "Action <name> has an invalid callers list. List one or more of \"signature\", \"key\" and \"session\", each once.", when: "callers is not a list, is empty, repeats a caller or names another one", source: "kernel" },
            { message: "Action <name> lists \"signature\" among its callers but declares no signature.", when: "callers names signature and the binding has no signature", source: "kernel" },
            { message: "Action <name> declares a signature, so its callers must include \"signature\".", when: "the binding has a signature and callers leaves it out", source: "kernel" },
            { message: "Action <name> signs with the secret <name>, which is not set for this project. Set it with secret_set before publishing.", when: "publishing, and the owner has not set the secret; the editor cannot see secrets, so only the Kernel refuses this", source: "kernel", preview: null },
            { message: "Actions <names> sign with the secret <name>, which is not set for this project. Set it with secret_set before publishing.", when: "publishing, and several actions sign with one secret the owner has not set", source: "kernel", preview: null },
            { message: "Actions <names> sign with the secrets <names>, which are not set for this project. Set each with secret_set before publishing.", when: "publishing, and more than one secret is unset; one refusal names every action and every secret", source: "kernel", preview: null },
            { message: "invalid-signature", when: "a signed route gets no signature, a malformed one, or one whose MAC does not match (401)", source: "kernel", status: 401, preview: null },
            { message: "stale-timestamp", when: "a signature's t is more than 300 seconds from the Kernel's clock; checked before the MAC, so it says nothing about the secret (401)", source: "kernel", status: 401, preview: null },
            { message: "authentication-required", when: "a route that takes an API key or a session, and no signature, gets neither (401)", source: "kernel", status: 401, preview: null },
            { message: "This route does not accept API keys.", when: "the route's callers leave out key and the request carries one (403)", source: "kernel", status: 403, preview: null },
            { message: "An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.", when: "a request to the runtime endpoint carries an API key (403)", source: "kernel", status: 403, preview: null },
            { message: "replayed-signature", when: "the same signature arrives a second time; a retry signs again with a new nonce (409)", source: "kernel", status: 409, preview: null },
            { message: "invalid-api-key", when: "Authorization carries an lbk_ key that is unknown or revoked (401)", source: "kernel", status: 401, preview: null },
            { message: "This API key may not call <name>.", when: "the key does not name the route's action (403)", source: "kernel", status: 403, preview: null },
            { message: "No route accepts POST on this path.", when: "a POST reaches a path no action binds (405)", source: "kernel", status: 405, preview: null },
            { message: "A route does not answer HEAD. Send GET.", when: "a HEAD reaches a path a query binds; it is refused before any SQL, with Allow: GET (405)", source: "kernel", status: 405, preview: null },
            { message: "The request body exceeds 64 KiB.", when: "a POST body is larger (413)", source: "kernel", status: 413, preview: null },
            { message: "The request body must be a JSON object.", when: "a POST body does not parse as JSON (400)", source: "kernel", status: 400, preview: null },
            { message: "Repeated query parameter: <key>", when: "a GET names one input twice (400)", source: "kernel", status: 400, preview: null },
            { message: "rate-limited", when: "a caller exceeds the action limits of the runtime endpoint, or its address sent 10 unknown or revoked API keys to the Deployment in a minute (429, with Retry-After)", source: "kernel", status: 429, preview: null },
          ],
          limits: [
            "a POST body of 64 KiB",
            "a signature's t within 300 seconds of the Kernel's clock, a nonce of 1 to 64 letters, digits, - and _, and each signature accepted once",
            "the runtime endpoint's action rate limits: 30 per 10 seconds per client address without a key or session, 120 per 10 seconds per user or key",
            "50 live API keys per Deployment",
          ],
        },
      ],
      notes: [
        "Only an action declared anonymous may bind a route. A route has three kinds of caller: a service that signs the request (signature), a program that sends Authorization: Bearer with an API key the owner issued (key), and a person signed in to the app, from the app's own origin (session). An actor binding yields the key's ID or the user's ID; a signed request has no actor.",
        "Without callers, a signed route takes the signature or a key, an unsigned route on a private app a key or a session, and an unsigned route on a public app anyone. A person whose session the route does not take must sign like any sender.",
        "An action whose route does not answer anyone answers only on that route: miniapp.actions.call refuses it. The app's own page calls such a route with fetch, which sends the session cookie.",
        "A signed route reads the declared header as t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256>. The MAC is keyed with the secret and covers t, the nonce, the method, the path, the canonical query string and the raw body, joined by newlines. The canonical query string is the pairs sorted by key, then value, written as application/x-www-form-urlencoded.",
        "The owner sets the secret with the secret_set admin tool before publishing. It never appears in actions.json, the Build Artifact or an export.",
        "GET /_littlebox/files/<id>, the path miniapp.files.url returns, answers a file the caller may read, with its own type. A person reads by the usual file rules, a visitor reads public files of a public app, and an API key reads the visibilities the owner named on it. A visitor without a session on a private app gets 401 authentication-required before any file is looked up; every other caller who may not read the file gets 404 File not found, the same as for a missing or deleted one.",
        "The editor preview serves no routes. Run them on a Development Kernel with littlebox dev, or on a Deployment.",
      ],
    },
  ],
} as const satisfies SdkContract;

/** Every call as "module.call", in contract order. */
export function contractCalls(contract: SdkContract = sdkContract): string[] {
  return contract.modules.flatMap((module) => module.calls.map((call) => `${module.name}.${call.name}`));
}
