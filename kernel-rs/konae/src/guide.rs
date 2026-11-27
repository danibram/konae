
use super::mcp::{admin_tools, Owner};
use super::runtime::runtime_tools;
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub(super) const TOPICS: [&str; 10] = [
    "rules",
    "pieces",
    "credentials",
    "development",
    "actions",
    "sdk",
    "publishing",
    "users",
    "portability",
    "refusals",
];

const SCOPES: [&str; 5] = ["read", "write", "publish", "manage_users", "export_data"];

pub(super) const READ_BACK: &[(&str, &str)] = &[
    ("project_create", "project_inspect"),
    ("project_sources_save", "project_sources_read"),
    ("project_delete", "projects_list"),
    ("database_execute", "database_execute"),
    ("secret_set", "secrets_list"),
    ("secret_delete", "secrets_list"),
    ("api_keys_create", "api_keys_list"),
    ("api_keys_revoke", "api_keys_list"),
    ("release_apply", "project_inspect"),
    ("app_invitations_create", "app_invitations_list"),
    ("app_invitation_resend", "app_invitations_list"),
    ("app_invitation_revoke", "app_invitations_list"),
    ("app_user_suspend", "app_users_list"),
    ("app_user_resume", "app_users_list"),
    ("app_user_enter", "auth_session"),
    ("domain_set", "domain_read"),
    ("domain_clear", "domain_read"),
    ("hostname_set", "project_inspect"),
    ("path_set", "path_read"),
    ("path_clear", "path_read"),
    ("maintenance_set", "maintenance_read"),
    ("contact_email_set", "contact_email_read"),
    ("module_set", "modules_read"),
    ("transfer_begin", "transfer_status"),
    ("transfer_complete", "transfer_status"),
    ("transfer_cancel", "transfer_status"),
    ("project_export", "transfer_read"),
    ("project_import", "project_inspect"),
    ("token_create", "tokens_list"),
    ("token_revoke", "tokens_list"),
    ("auth_logout", "auth_session"),
    ("auth_signup", "auth_session"),
    ("auth_verify", "auth_session"),
    ("auth_recover", "mail_status"),
    ("auth_reset", "auth_session"),
    ("files_upload", "files_list"),
    ("files_update", "files_list"),
    ("files_delete", "files_list"),
    ("actions_invoke", "actions_invoke"),
];

const SDK_LOADER: &str = r#"<script>(function(){var m=/^\/p\/([^\/]+)(\/|$)/.exec(location.pathname);document.write('<script src="'+(m?'/p/'+m[1]:'')+'/runtime.js"><\/script>')})();</script>"#;

pub(super) const READ_BACK_IN_ANSWER: &[(&str, &str)] = &[
    ("function_candidate_check", "Its accepted response names scratchRevision and buildFingerprint, binding the check to the current candidate and exact uploaded bytes. It changes no app state."),
    ("transfer_write", "Its nextOffset is where the next block starts. Write the next block there; call transfer_status only to resume after an interrupted call, and transfer_complete checks the whole upload."),
    ("release_plan", "It changes nothing live. Its planId, expectedRevision and schema are the plan; pass that planId to release_apply. Calling release_plan again makes another plan with a new planId."),
];

pub(super) const READ_BACK_EXPECT: &[(&str, &str)] = &[
    ("transfer_cancel", "transfer_status answers \"Transfer not found or expired.\" That answer is the cancel confirmed: the transfer and its bytes are gone."),
];

pub(super) const READ_BACK_GAPS: &[(&str, &str)] = &[];

pub(super) fn topic(name: &str) -> Result<Value> {
    Ok(match name {
        "rules" => json!({
            "yardstick": "Konae lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.",
            "decide": "A new capability belongs in the product only when the Validation Gate and the Kernel refuse it with the same words, and an export of the app folder can carry the app's accounts, data and rules. Anything else is operator work on the installation, not an app feature.",
            "words": "An owner has a Konae account and manages apps through this MCP or the editor. An app user has an account in one app, kept by the Kernel. The app folder is the app: konae.json, app/, dist/ and data/, the same on a laptop, on this installation and in an export.",
            "sameChecks": "A query that reads rows names a LIMIT in the editor and on the Kernel that leaves with the app. Who may open the app, and who may read a row, stay decisions of the app.",
            "modules": "modules_read answers every module's state, switch, options, permission and current behavior. module_set needs the module's contract permission, takes the state read as expected, and is read back with modules_read. A stale state is refused with module.changed. The editor and build agent use these same tools. Both tools need a published Deployment. Before first publish, the editor shows the project's module state read-only; the first release carries that state.",
            "firstRun": {
                "cli": [
                    "Run konae init and konae dev, which need no credential",
                    "Run konae login --browser, then konae publish",
                    "Read the published configuration with project_inspect, open the Deployment URL, sign in if it is private, and call a real Server Action through the runtime MCP endpoint with the appropriate app session"
                ],
                "mcp": [
                    "Run konae init and konae dev, which need no credential",
                    "Run konae login --browser, then konae publish",
                    "Read the published configuration with project_inspect, open the Deployment URL, sign in if it is private, and call a real Server Action through the runtime MCP endpoint with the appropriate app session"
                ],
                "after": "If the first run grows past these steps, the product got harder. Prefer fixing the path over adding a guide page."
            },
            "readBack": {
                "rule": "Every tool that writes has a way to observe its effect through the same door. map names the tool to call after the write, before retrying or telling the owner it worked; inAnswer names the writes whose own answer is the observation, so no second call follows them. A service_timeout or interrupted call must be verified the same way.",
                "map": Value::Object(READ_BACK.iter().map(|(write, read)| ((*write).to_owned(), json!(*read))).collect()),
                "inAnswer": Value::Object(READ_BACK_IN_ANSWER.iter().map(|(write, read)| ((*write).to_owned(), json!(*read))).collect()),
                "expect": Value::Object(READ_BACK_EXPECT.iter().map(|(write, answer)| ((*write).to_owned(), json!(*answer))).collect()),
                "gaps": Value::Object(READ_BACK_GAPS.iter().map(|(write, close)| ((*write).to_owned(), json!(*close))).collect()),
                "staging": "Bytes in an open transfer are not a Deployment. An upload is transfer_begin, one transfer_write per block of chunkBytes, then transfer_complete, whose read-back is transfer_status. project_inspect is the read-back of a release that consumed them."
            },
            "capabilities": "session_inspect answers can for the scopes this credential holds, and createTokens when it is the owner's browser session. Decide from can, not from memory of which flag login took.",
            "divergences": [
                {"where": "editor preview vs published Deployment", "fact": "The preview applies no Content Security Policy. Inline event handlers, other-origin fetches and WebAssembly can still run there; a published app and bun run konae -- dev refuse them. Its script-src is 'self' plus the hash of each inline script, with no 'wasm-unsafe-eval' and no 'unsafe-eval', so WebAssembly.compile and WebAssembly.instantiate fail with \"Compiling or instantiating WebAssembly module violates the following Content Security policy directive\", and eval and new Function fail too. Attach listeners in JavaScript, keep scripts and fetches on the app's own origin, and pick libraries that run without WebAssembly or eval."},
                {"where": "editor preview vs Deployment", "fact": "The preview has no HTTP routes, schedule timer, notice email, Usage store or call outbox. Use a published Deployment or the local test controls. Use konae test schedule <action> for a mutation or konae test schedule cron:<id> for a Function in konae dev."},
                {"where": "editor preview vs Kernel", "fact": "The preview opens one srcDoc document in an iframe. It has no app URL or page history, so it cannot resolve declared page paths or populate miniapp.page.params. Try page paths on konae dev or a Deployment. It writes no head tags for row values."},
                {"where": "editor preview vs Kernel", "fact": "The editor preview refuses a file over 2 MiB. A Kernel refuses at its own KONAE_FILE_MAX_BYTES, 2 MiB unless the operator raised it, and names the number. konae dev applies the same 2 MiB per file, and KONAE_FILE_MAX_BYTES raises it there. Its quota for all files is 1 TiB unless KONAE_FILES_QUOTA_BYTES names another."},
                {"where": "editor preview vs Deployment", "fact": "The editor preview has no Google sign-in. miniapp.auth.signInWithGoogle() refuses there; try it on a published Deployment once the owner has set the app's Google client and turned Google on."},
                {"where": "editor preview vs konae dev", "fact": "The editor preview writes no canonical, card, robots.txt or sitemap. konae dev writes them using the local host as the address."},
                {"where": "editor build vs konae build", "fact": "The editor's build reads npm packages from esm.sh; konae build and an app folder's npm run build read them from node_modules. Both use the exact versions in package.json, but the bundles are not byte-identical."},
                {"where": "Validation Gate vs publish builders", "fact": "The Gate checks the schema and the declarations in actions.json, functions.json and crons.json. It refuses a Capsule that uses miniapp.files while the files module is off. It also refuses a Capsule whose pages are all whole documents that load no script, as the build does. A page that names a missing file fails at build or release, not at validate_capsule. Check every path you write."},
                {"where": "editor preview vs Kernel", "fact": "The editor preview re-runs schema.sql on its own test rows and never changes a table it already has. A Kernel derives the schema change at publish. When the preview lacks a table or column schema.sql declares, the Gate warns and passes. Try such a change on bun run konae -- dev or a published Deployment."},
                {"where": "konae dev vs Deployment", "fact": "konae dev uses test mode and sends no email. Account email and notices land in .konae/kernel/mail-testbox.jsonl. It captures calls, pauses schedule timers, refuses Google sign-in and blocks browser connections to other origins; npm scripts and shell commands are not sandboxed. konae events lists local effects."}
            ]
        }),
        "pieces" => json!({
            "rule": "Pick the smallest piece that does the job. Prefer a declaration inside the Capsule over a new platform resource, a second process or a second database door.",
            "choose": [
                {"need": "Page and rows the app owns", "use": "Capsule + Server Actions", "not": "Raw SQL from the browser"},
                {"need": "Visitor or app user reads or writes a row", "use": "Named Server Action with access and actor bindings", "not": "Owner database_execute in the app UI"},
                {"need": "Mutation matched no row", "use": "guard on that mutation", "not": "A second check statement"},
                {"need": "Email when a row changes", "use": "notify on that mutation", "not": "Browser mail or a call action for mail"},
                {"need": "Run one SQL mutation on an interval or at a UTC time", "use": "schedule on a no-input mutation", "not": "External cron"},
                {"need": "Run scheduled JavaScript or use a cron expression", "use": "crons.json and a Function with scheduled access", "not": "External cron"},
                {"need": "Owner-facing occurrence", "use": "signal on a mutation, or kind: signal", "not": "A side table only the agent reads"},
                {"need": "Another service calls this app", "use": "Turn on http with module_set, then declare it on an anonymous action with callers / signature / key", "not": "A call action (that is outbound)"},
                {"need": "Code between an HTTP request and the rows", "use": "A server function in functions.json, with its actions allowlist", "not": "A Server Action with string-built SQL, or a call action"},
                {"need": "This app calls another host", "use": "Turn on calls with module_set, then declare kind: call", "not": "fetch of a secret from the browser"},
                {"need": "Uploaded files", "use": "miniapp.files", "not": "Large bytes in a SQLite column"},
                {"need": "Issue a private PDF", "use": "Documents, an issue mutation and a named download query", "not": "A public Files upload"},
                {"need": "Give an external MCP client read access to app rows", "use": "App MCP and an authenticated query with an mcp declaration", "not": "Owner MCP credentials"},
                {"need": "Accept file versions from a device or test fixture", "use": "File intake with a declared source and optional Actions consumer", "not": "A browser Files upload"},
                {"need": "Receive device uploads over FTPS", "use": "The installation's FTPS listener bound to a published File intake source", "not": "An app module switch for the listener"},
                {"need": "Every signed-in user reads every uploaded file", "use": "files.fileAccess set to app-users", "not": "Each upload as public"},
                {"need": "Who may create an app account", "use": "Signup Policy and invitations", "not": "A homemade users table that bypasses Runtime Actor"},
                {"need": "An administrator inside the app", "use": "A role column in the app's own table, checked in SQL", "not": "An owner account inside the app"},
                {"need": "Turn an app capability on or set its options", "use": "modules_read and module_set, then read it back with modules_read", "not": "A separate tool or a setting in the Capsule"},
                {"need": "Be listed in search", "use": "Turn on seo with module_set, after modules_read", "not": "Assume a public app is listed"},
                {"need": "A short link on the installation's domain", "use": "path address (path_set)", "not": "A custom domain when DNS is not yours"},
                {"need": "Take the app elsewhere", "use": "export and import of the app folder", "not": "Copy (moves the installation)"},
                {"need": "Debug or rehearse a source change against published rows", "use": "konae pull --data-only, dev --reset, local dev, then rehearse", "not": "Publishing to find out"},
                {"need": "Put data from the folder, or from another system, into the published app", "use": "konae push (an import of the folder's data/, after a backup)", "not": "A migration script that drives transfers and imports by hand"},
                {"need": "See the email, calls, scheduled runs and Signals of a local run", "use": "konae events", "not": "Reading the Kernel's files by hand"}
            ],
            "unit": "The app folder is the app: konae.json, app/, dist/ and data/ with its database, accounts, files and Usage. The Kernel keeps it under apps/<id>/, and an export hands the same folder over.",
            "moduleSetup": {
                "published": [
                    "Read session_inspect.can and modules_read { projectId }. The three app switches need publish. Turning on auth needs manage_users.",
                    "Enable dependencies first. Call module_set { projectId, module, on: true, expected }, with modules.<name>.state from modules_read as expected. Read each change back with modules_read. The editor asks the owner to confirm the change.",
                    "Save app sources and read them back. In the editor, validate the scratch files and have the owner apply them. Publish and use project_inspect to read the active deployment before running the feature check."
                ],
                "firstPublish": "For a first CLI publish, set the app switches in konae.json. If the web editor project has a required switch off before its first publish, publish the base app first, then follow the published procedure before adding the declaration.",
                "documents": {
                    "requires": ["actions", "auth"],
                    "declare": "Enable documents. In actions.json, put issue on an authenticated mutation with a string issue key. Supply a snapshot and a named authenticated query that checks actor.id and issueKey. Check row authority on the mutation too.",
                    "operator": "Both images include Typst 0.14.2. On a native Kernel, install this version or set KONAE_DOCUMENTS_TYPST_BIN to its executable path.",
                    "verify": "Run the mutation as its app user and open /_konae/documents/<issue-key> on the app address, including its base path. Verify a PDF for the allowed user and 404 for a signed-in user the query excludes."
                },
                "mcp": {
                    "requires": ["actions", "auth"],
                    "declare": "Enable mcp and publish an authenticated query with mcp.description. Its input bindings and app row rule also govern the tool.",
                    "operator": "Register the public client in <KONAE_DATA_DIR>/mcp-app-clients.json using the app ID from project_inspect and the exact redirect URIs. The client uses app OAuth with PKCE S256.",
                    "verify": "Use the app address, including its base path. Grant the selected query as an app user, then use tools/list and tools/call on /_konae/mcp to check the tool and its rows."
                },
                "file-intake": {
                    "requires": ["actions"],
                    "declare": "Enable file-intake. Save file-intake.json in the Capsule, under app/ in a folder. Declare source IDs and any consumers. Consumers run declared SQL mutations with version metadata.",
                    "verify": "With the local dev server stopped, use konae test intake <source> --path <relative-path> --file <completed-file> --delivery-key <key>. Repeat the same input and check for the same versionId and created: false. Restart konae dev to pick up pending consumers. Read their changes through an authorized query."
                },
                "ftps": {
                    "operator": "Publish the File intake source. Create <KONAE_DATA_DIR>/ftps.json on the installation with mode 0600. Bind each credential to the app ID and declared source ID. Set the certificate, key, bind address and passive ports. Restart the Kernel. Credentials belong in this operator file.",
                    "verify": "Run kernel ftps status with the serving data directory, then upload a test file using explicit TLS on both connections. Verify the 226 reply and the app consumer's rows if one is declared."
                }
            },
            "cronJobs": "Read the functions module in the sdk topic for the declaration. Read crons.json and functions.json through project_sources_read. After changing them with project_sources_save, use project_sources_read again to confirm the saved sources. Publishing installs the schedule and enables Functions. It also enables Actions when the cron target calls them. Read the published job IDs under modules.functions.current.crons with modules_read."
        }),
        "credentials" => json!({
            "login": "bun run konae -- login --dir <folder> --url <installation URL> reads KONAE_EMAIL and KONAE_PASSWORD, and asks for either one that is unset when it runs in a terminal. Add KONAE_INVITATION_CODE, the code the operator issued with konae owner-code-create, to create the account, or KONAE_SETUP_KEY for an installation's first owner. It saves a 24-hour token with read, write and publish, and the installation's address, in <folder>/.konae/credentials.json, which publish, pull, push, secret, domain, hostname, tool, export --with-data, and sql and events with --prod read. KONAE_MCP_TOKEN overrides that file. On a Kernel that verifies email, creating the account sends a link; on a Kernel on the same machine, --verify-from <data folder>/mailbox.jsonl answers it with KONAE_PASSWORD in the same run.",
            "browser": "When nobody has the owner's password, run bun run konae -- login --browser --dir <folder> --url <installation URL>, with the same flags. It prints an address and opens it; hand the address to the owner, who opens it in a browser on the same machine, signs in and allows access within 5 minutes. It saves the same 24-hour token in the same file, one hour under Clerk. No password or token passes through the terminal or the agent.",
            "secrets": "bun run konae -- secret set <name> --dir <folder> [--use route|call|google] reads the value from stdin, so it stays out of the agent's context and argv. The owner pipes it in or types it at the hidden prompt. It reads the name back with secrets_list. secret list prints names and uses, never values. Prefer it to secret_set with the value as an argument.",
            "flags": {"--manage-users": "adds manage_users", "--export-data": "adds export_data"},
            "projectScope": "A project-scoped credential can act only on the existing project IDs it names, within the permissions it holds. It cannot create a project. Create projects from the owner's browser session or publish a folder with konae.",
            "oauth": "An MCP client with OAuth reads /.well-known/oauth-authorization-server, registers at /oauth/register, and sends the owner to /oauth/authorize with PKCE S256, resource <origin>/mcp and a scope of read, write, publish and manage_users. A client that redirects to http://127.0.0.1, localhost or [::1] gets a 24-hour token and may also ask for export_data; any other client gets a one-hour token without export_data. Under Clerk every token lasts one hour. There is no refresh token.",
            "permissions": {
                "none": "platform_describe, platform_guide, session_inspect and mail_status",
                "read": "projects_list, project_inspect (including deployment.signupPolicy), project_sources_read, modules_read, domain_read, path_read, maintenance_read, contact_email_read, secrets_list, api_keys_list, usage_summary, signals_read, usage_overview, calls_read, notices_read and database_execute reads",
                "write": "project_create, project_sources_save, project_delete and database_execute changes",
                "publish": "release_plan, release_apply, secret_set, secret_delete, api_keys_create, api_keys_revoke, hostname_set, domain_set, domain_clear, path_set, path_clear, maintenance_set, contact_email_set, module_set for modules with publish permission, project_import, and initialSignupPolicy on a first release",
                "module_set": "Changes a module whose registry permission is held by the credential",
                "modules_read": "Reads module state and options; requires read",
                "manage_users": "module_set for auth, app_users_list, app_user_suspend, app_user_resume, app_user_enter from the owner's browser session, and the app_invitations tools",
                "export_data": "project_export and reading its downloads",
                "ownerSession": "tokens_list, token_create, token_revoke and app_user_enter"
            },
            "refusals": "A tool this credential may not call answers 'Unknown tool or insufficient permission.' and names the permission it needs. A delegated token cannot create tokens or add permissions to itself, so run login again with the flag. session_inspect lists scopes and can for what the current credential holds. An expired token answers 401 with its expiry time and offers the recovery steps for that credential; use konae login for a new CLI credential, reconnect the MCP client, or sign in as the owner."
        }),
        "development" => json!({
            "commands": [
                "bun run konae -- init --dir /tmp/my-app --name 'My app' [--template tasks|notes|members|form|webhook|digest|gallery]",
                "bun run konae -- dev --dir /tmp/my-app [--reset]",
                "bun run konae -- build --dir /tmp/my-app",
                "bun run konae -- test --dir /tmp/my-app schedule cron:<id>",
                "bun run konae -- contract --dir /tmp/my-app",
                "bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 [--browser] [--manage-users] [--export-data]",
                "bun run konae -- secret set <name> --dir /tmp/my-app [--use route|call|google], with the value on stdin",
                "bun run konae -- publish --dir /tmp/my-app",
                "bun run konae -- hostname --dir /tmp/my-app --deployment <id> [--set <name>]",
                "bun run konae -- pull --dir /tmp/my-app [--data-only]",
                "bun run konae -- dev --dir /tmp/my-app --reset",
                "bun run konae -- test --dir /tmp/my-app status|calls|reply|schedule|scenario",
                "bun run konae -- rehearse --dir /tmp/my-app [--confirm-schema <confirmation>]",
                "bun run konae -- schema fingerprint --dir /tmp/my-app [--data] [--json]",
                "bun run konae -- push --dir /tmp/my-app [--yes]",
                "bun run konae -- sql --dir /tmp/my-app [--prod] '<statement>'",
                "bun run konae -- events --dir /tmp/my-app [--follow] [--json] [--prod]",
                "bun run konae -- tool <name> ['<json arguments>'] [--stdin <key>] --dir /tmp/my-app",
                "bun run konae -- conform --url <app URL> --contract /tmp/my-app/action-contract.json --token <app session> --key <API key> --secret <name>=<value>"
            ],
            "credentials": "init, dev, build and contract, and sql and events without --prod, need no credential. login saves the credential and the installation's address in the folder, and every other command but conform and export without --with-data reads it from there or from KONAE_MCP_TOKEN. tool <name> calls any owner tool by name and prints its answer, and --stdin <key> reads one argument from standard input, for a secret's value. domain and tool talk to --url or the installation login saved, as publish does. The first publish registers the project with project_create. The credentials topic lists the permissions. Never put a token in app source.",
            "liveData": "pull needs export_data and downloads app/, dist/, konae.json and published data into .konae/pull. pull --data-only refreshes the data snapshot while preserving local sources, settings and their recorded source baseline. dev --reset imports that snapshot after backing up data/ and local queues; it retires unfinished local effects and reports execution and snapshot IDs, backup path and counts. Stop dev before reset. Then run dev without --reset to keep the local data. rehearse builds current sources and checks import against the snapshot in scratch, ignoring hand-fixed local rows. It does not change the Deployment. publish --plan reviews current sources against current production rows; publish sends sources and dist/ but never local data. push is a separate data replacement after a backup and needs --yes.",
            "events": "events lists local email (saved in .konae/kernel/mail-testbox.jsonl), captured calls, scheduled runs, routes and Signals. The page dev serves on the next port shows the same list. Test mode captures call actions without network access, pauses schedule timers, refuses Google sign-in and blocks browser connections to other origins. npm scripts and shell commands are not sandboxed. Use test status, calls, reply, schedule and scenario for operator controls. Calls show response numbers and nextAttemptAt; submit one synthetic HTTP response or transport error for each attempt. An interrupted then continuation has unknown outcome and is not replayed; reset local data and retest.",
            "start": "In an empty folder, either run init, or call project_create with a name and an optional template and write the files it returns into app/ plus konae.json {id, name, slug, visibility, signupPolicy, signInMethods, kernel, manifest}. Both give the same files. init also writes AGENTS.md, action-contract.md, compose.yaml, .env.example and a package.json whose npm run build writes dist/. Then dev runs it locally and publish sends it to the installation.",
            "isolation": "dev runs the kernel binary on the folder with its own data/, no owner and no MCP, and does not proxy production. It keeps the Signup Policy konae.json names; on an app that is not Open and holds no account, dev invites test@example.com and prints the link that opens the invitation. For another address, use the kernel binary's invite command, as dev prints it.",
            "sources": "publish sends every file of app/, text or binary, without dot entries or node_modules, and the built dist/. app/ holds at most 1000 files and 20 MiB once base64 is decoded, each path at most 240 bytes. Every publish writes that app/ beside dist/. Existing apps require observedSourcesRevision from project_inspect or project_sources_read; a disagreement refuses with sources.revision-conflict and says to run konae pull --dir <folder>. release_apply checks again after the plan. Successful releases increment the source revision. The editor waits for saves and names the revision they wrote.",
            "functionPreparation": "When the Capsule declares functions, the editor builds the candidate Server Artifact, uploads its exact JSON as a completed transfer scoped to the project, reads transfer_status back, then calls function_candidate_check. The tool needs write and prepares only the uploaded bytes. Its answer names the scratch revision and upload digest. A disconnected check leaves Apply pending; reconnect and validate the current candidate again.",
            "contract": "contract writes action-contract.json, its schema, action-contract.md and actions.d.ts, which types miniapp.actions.call, from actions.json and schema.sql. conform calls every action on a Deployment and checks the answers against that file, so point it at a test Deployment. The owner reads what ran with usage_summary and signals_read."
        }),
        "actions" => json!({"filename":"actions.json","format":{"version":1,"actions":{"items.list":{"kind":"query","access":"authenticated","statement":"SELECT id,title FROM items WHERE user_id=?1 LIMIT 50","bindings":[{"source":"actor","field":"id"}]},"items.create":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO items(title,user_id) VALUES(?1,?2)","bindings":[{"source":"input","key":"title","type":"string","maxLength":200},{"source":"actor","field":"id"}]}}},"rules":"SQL and typed bindings stay server-side. Declare schema in schema.sql for artifact publication. Runtime calls only send the declared action name and input. Add nullable:true to an input binding to bind omitted values or explicit null as SQL NULL; supplied strings remain non-empty and all supplied values retain their type checks. Mutations return changes and lastInsertId, so read rows with a query instead of mutation RETURNING, which publication rejects. A query that reads rows needs a top-level LIMIT. A total with count, sum, avg, min or max and no GROUP BY does not. An anonymous action may add http:{method:'GET' for a query or 'POST' for a mutation,path,signature?:{scheme?:'konae'|'hmac-body'|'timestamped'|'svix',header,secret,...},fields?:{inputKey:'$.data.object.id'},callers?:['signature'|'key'|'session']} to answer on that path of its Deployment host; a provider's scheme and fields take a webhook as GitHub, Shopify, Stripe or Resend send it; an action whose route does not answer anyone answers only there, not through actions_invoke. The sdk topic's http module describes routes, callers, signatures, API keys and secret_set. A mutation may add guard:'<sentence>' so a statement that changes no row is refused with it instead of answering changes 0. A mutation may add notify:{to:'owner' or 'actor' or {column:'<name>'},changes?:['<column>'],subject,body} to queue email when it changes a row: to the app's contact email in konae.json, which the owner sets in App settings or with contact_email_set and a first publish from Konae fills with the owner's email when Konae knows it, to the calling account once it has proved its email, or to each proven account of this app whose ID a changed row holds in that column, with only its own rows. The templates take {input.<key>} for its input bindings, {actor.id}, {kernel.now}, {changes.count} and, in the body, {changes}, which lists the changed rows' changes columns; it never sends to an address from the input or a row. Usage counts each notice under notify, and the owner reads each queued notice's state with notices_read. A mutation may add signal:{name:'order.placed',properties:[input keys or 'actor']} to record a Signal when it changes a row, and kind:'signal' with access, bindings and signal but no statement records one without SQL. The owner reads them with signals_read and Usage with usage_summary. kind:'call' with access, input bindings only, method:'GET'|'POST', url, body:{bodyKey:'inputKey' or {value:constant}}, auth?:{secret,header?}|{key}, then?:{action,inputs:{key:'/status'|'/body/...'|'/input/...'}} queues a request the Kernel sends from its own process and answers {callId,queued:true}; the calls module of the sdk topic gives the address rules, and the owner reads the outbox with calls_read."}),
        "sdk" => serde_json::from_str::<Value>(include_str!("sdk-guide.json"))?,
        "publishing" => json!({
            "schema_transforms": "An optional app/schema-transforms.json carries existing rows through a rename or backfill. Its version 1 transitions form an append-only linear chain with id, predecessor, before and after schema fingerprints, maxRows and statements. Read fingerprints with konae schema fingerprint, using --data for the observed database. Preserve applied transitions unchanged. Write explicit ALTER TABLE RENAME steps to keep values, and rehearse against pulled data before publish. The editor Gate checks shape and supported SQL; the Kernel checks history, fingerprints, measured row counts, final schema, actions and foreign keys on a staged database. INSERT and UPDATE steps need confirmation. The guide docs/guides/schema-transformations.md gives the format and limits; the editor offers it with read at /reference/schema-transforms.md.",
            "steps": [
                "Use the downloadable konae CLI to publish: run konae init in a folder, run konae dev, then konae login --browser and konae publish. Read the published configuration with project_inspect, open the Deployment URL, sign in if the app is private, and call a real Server Action through its runtime MCP endpoint with the appropriate app session. The tool sequence below documents the internal owner protocol used by existing integrations and tests.",
                "Create project identity with project_create { projectId, name, slug, manifest }. Keep its id, slug and manifest locally.",
                "Set every secret an action's http signature names with secret_set { projectId, name, value }. A release that signs with an unset secret is refused and names each one.",
                "Write the release JSON that payload describes, with the SDK that sdk describes, and the sources JSON that payload.sources describes.",
                "Upload each as its own upload: transfer_begin { size, sha256, projectId } with the byte size and SHA-256 of the UTF-8 JSON answers transferId and chunkBytes. Call transfer_write { transferId, offset, data } with base64 blocks of at most chunkBytes, each at the nextOffset the previous write answered, then transfer_complete { transferId }. A JSON smaller than chunkBytes is one write. Call transfer_status only to resume an interrupted upload. Never name a component; that is for the parts of an export.",
                "Call release_plan { transferId, sourcesTransferId, observedSourcesRevision } once. Existing apps need the source revision project_inspect or project_sources_read returned to this caller. Inspect planId, expectedRevision, expectedSourcesRevision and schema, then call release_apply { planId }. release_apply refuses if the source revision changed after planning. A second release_plan makes another plan with a new planId; it is not a read-back.",
                "schema is the change the Kernel derived from schema.sql to the live database, with statements, losses and confirmation. A new table, column or index applies. When losses is not empty (a dropped table, a dropped column, a changed column type), show the owner each loss's sentence and pass confirmation as confirmSchema only after the owner agrees. Without it release_apply refuses with schema.confirmation-required and changes nothing.",
                "A successful release_apply consumes the release, sources and referenced initial-package uploads after its receipt commits. Failed or unconfirmed plans keep their uploads for retry. A caller's final transfer_cancel is safe and idempotent; transfer_status then answers \"Transfer not found or expired.\" Read the live Deployment with project_inspect."
            ],
            "payload": {
                "shape": {"project": {"id": "<projectId>", "name": "<name>", "slug": "<slug>", "manifest": {"version": 1, "visibility": "private"}}, "visibility": "private", "artifact": {"version": 1, "entrypoint": "index.html", "files": {"index.html": "<!doctype html>...", "runtime.js": "<the SDK>", "logo.png": "<base64>"}, "encodings": {"logo.png": "base64"}}, "initialSignupPolicy": {"mode": "invitation", "allowlist": []}},
                "project": "id, name and slug as project_create answered them. manifest is konae.json's manifest without signupPolicy, such as routing and connects.",
                "sources": "The sources upload is {\"files\": {\"actions.json\": \"<text>\", \"schema.sql\": \"<text>\", \"index.html\": \"<text>\", \"public/logo.png\": \"<base64>\"}, \"encodings\": {\"public/logo.png\": \"base64\"}}, every file of app/ by its path inside app/. A file that is UTF-8 text is its text; any other file is base64, and encodings names it with the value base64, as in the artifact. Every release writes these files into app/ with dist/. An app with no Server Actions or tables may omit actions.json and schema.sql. The Kernel derives the schema change from schema.sql when the app has one. project_import accepts this separate sources upload as sourcesTransferId.",
                "visibility": "public or private.",
                "artifact": "What the Deployment serves, dist/ after a build: version 1, entrypoint index.html, and files by their path inside dist/. index.html is required. A file that is UTF-8 text is its text; any other file, such as an image, is base64 and encodings names it with the value base64, the only encoding. Omit encodings when every file is text. At most 1000 files and 16 MiB once base64 is decoded. A path under _konae/, oauth/ or .well-known/, or the path mcp, is refused.",
                "optional": "initialSignupPolicy {mode, allowlist} on the first release only; contactEmail, the address notify to owner reaches; initialPackage on a first release that imports an export.",
                "limit": "The release JSON is at most 26 MB. The sources upload is at most 32 MiB, and the files of app/ at most 1000 files and 20 MiB once base64 is decoded. A refusal names the size and the limit."
            },
            "sdk": format!("The Kernel does not add the SDK; the artifact carries it. konae build inlines Konae's src/mcp/runtime.js in a script tag with the data-konae-runtime attribute on a fragment page, and a whole-document page loads /runtime.js, an artifact file with the same source. Without a shell, put that file at runtime.js in artifact.files, taken from the Konae version the installation runs, and load it in each page's head before the page's own scripts with the loader konae build writes, which finds runtime.js on the app's hostname and on a path address alike: {SDK_LOADER}"),
            "cli": "bun run konae -- publish --dir <folder> runs these steps with the credential from login and sends signupPolicy from konae.json as initialSignupPolicy on the first release.",
            "concurrency": "An obsolete plan is rejected. Repeating an applied plan returns the same receipt. contact_email_set, domain_set, hostname_set, path_set, maintenance_set and module_set accept expected from their read tool; setting.changed omits the current value unless the credential also holds read.",
            "access": "A first release may add initialSignupPolicy {mode, allowlist} with mode open, invitation, closed or allowlist; the Kernel installs it before the app serves. When the owner's request leaves access open for an app with accounts, propose visibility private and mode invitation. Later changes need manage_users and module_set on auth, then modules_read. Invite people with app_invitations_create only when the owner asks, because it sends email.",
            "import": "A first release may also carry initialPackage to start the Deployment with an exported app's data. project_import replaces an existing app's sources, data or both; the portability topic has the steps.",
            "domain": "domain_set { projectId, hostname } adds a custom domain next to the generated hostname and needs publish. Its first answer names a TXT record, _konae.<hostname>, and the token it must hold. Once the record is at the DNS provider and the hostname points at the installation's proxy, which terminates TLS and forwards Host unchanged, call domain_set again: the Kernel looks the record up and the alias serves. domain_read shows the state and domain_clear removes it. bun run konae -- domain runs the same steps.",
            "hostname": "Konae names every app under the app suffix. The first release takes <slug>.<app suffix>, or <slug>-<code>.<app suffix> when another app holds the slug, and later releases keep it. release_plan answers that name as hostname and url, with firstRelease true, before the first release; show it to the owner before they configure Google or DNS. domain_set refuses any name under the suffix. hostname_set { projectId, name } moves the generated hostname to <name>.<app suffix> when no other app on the installation holds or held it, and needs publish. The old name answers with a 308 redirect to the same path on the new one and stays the app's. App users sign in again on the new name, and an app with Google sign-in registers https://<name>.<app suffix>/_konae/auth/google/callback, the googleCallback the answer names, in its Google client. Services that call the app's HTTP routes need the new address. project_inspect shows the new url. The name belongs to the installation and does not travel in an export.",
            "maintenance": "maintenance_set { projectId, on } puts the app in maintenance or takes it out and needs publish. Paused, the app serves a maintenance page, refuses writes, signups and sign-ins, and holds its email, calls and schedule; owner tools that would change it are refused except maintenance_set and project_import. maintenance_read { projectId } answers { maintenance, since, unreadable, installation }. Move an app with no lost write by putting it in maintenance, exporting it, and importing the folder elsewhere.",
            "path": "path_set { projectId, enabled, name?, acceptSharedOrigin? } answers the app on <domain>/p/<id>, and on <domain>/p/<name> once it has a name. The domain is KONAE_DOMAIN. An app with accounts or a private app needs acceptSharedOrigin true, because every path and the home share one origin, and any other app on the domain can use the sessions of this app's users. The app whose enabled path holds the name konae-home also answers <domain>/ and every path there outside /p/; with none, <domain>/ answers not found. path_read returns the state and the addresses; path_clear turns the path off and removes the name. Both need publish. Read back with path_read after a change."
        }),
        "users" => json!({
            "app_administrators": "An app keeps roles in its own table, not in Kernel accounts. To appoint a person at the owner's request, inspect session_inspect.can, resolve the exact email with app_users_list including nextCursor pages and use the stable account ID. An editable display name never proves account identity. Read that ID's role with database_execute, change only that ID with the owner's authorization and read it back. In the editor the mutation waits in its confirmation dialog; the owner need not write SQL. If the account or its profile is absent, explain how to create it or sign in once.",
            "permission": "manage_users, which login adds with --manage-users. modules_read needs read.",
            "policy": "module_set { projectId, module: \"auth\", options: { signupPolicy: { mode, allowlist } } } sets open, invitation, closed or allowlist and its separate entries. Read it back with modules_read. A publish never changes the policy after the first release.",
            "google": "Each app has its own Google OAuth client, which the owner makes in their own Google console. modules_read { projectId } returns redirectUris, the callbacks to register in that client: the app's own host, a verified custom domain, and <domain>/p/<id> while the path address is on (the name address uses the id's callback; the home address offers no Google), and googleClient { clientIdSet, clientSecretSet }. secret_set { projectId, name: \"google_client_id\" or \"google_client_secret\", value, use: \"google\" } sets each half; the value is never returned and no route or call reads it. Then module_set { projectId, module: \"auth\", options: { signInMethods: [\"password\", \"google\"] } } adds Continue with Google to the sign-in page; [\"password\"] removes it. Password cannot be turned off. Read back with modules_read. While either half is unset it refuses with \"Set this app's Google client ID and client secret before turning Google on.\", and while Google is on secret_delete refuses either half. A Google sign-in reaches the app account that holds the same email, made with a password or with Google, and Signup Policy decides whether an email without an account gets one. Google proves only an address it manages: a gmail.com address, or one whose Workspace domain the token names in hd; any other address is refused and signs in with email and password. An active account whose email nobody proved is taken back for the person Google vouches for: its password and sessions stop working. An export carries signInMethods and the accounts, never the client: on another installation the owner sets the two secrets and registers that host's redirect URI, and until then the sign-in page offers only email and password and modules_read returns a notice.",
            "invite": [
                "Call mail_status: inviting needs sending true. When it answers delivery local-mailbox, the transport is development and no email leaves the server.",
                "app_invitations_create { projectId, emails, idempotencyKey } emails each address a seven-day link to /_konae/invitation#invitation=<token> on the app's host. Call it only when the owner asked for those invitations. Under the development transport each result answers delivery local-mailbox and the link as acceptUrl, once; hand it to the person. A retry with the same key and app_invitations_list say queued and carry no link, and app_invitation_resend issues a new one.",
                "The person opens the link and chooses a password; the page posts it to /_konae/auth/accept and signs them in.",
                "app_invitations_list shows each invitation's status; app_invitation_resend sends a new link and app_invitation_revoke ends one."
            ],
            "accounts": "app_users_list, app_user_suspend and app_user_resume. Suspension ends sessions and keeps the account's rows.",
            "enter": "app_user_enter { projectId, userId } answers a link on the app's own address that works once, for two minutes, and signs the browser that opens it in as that active account. It needs manage_users and the owner's own browser session, so an agent cannot call it; the editor's Users view has the button. The audit log records who entered as whom, and Usage and Signals do not count what that session does.",
            "roles": "An app account has no role the Kernel keeps. An app that needs an administrator keeps roles in its own table, such as profiles(user_id, role), checks them in SQL with the actor binding { source: actor, field: id }, and the owner names the first one with database_execute sql. auth_session answers role member for every signed-in account and anonymous for a visitor."
        }),
        "portability" => json!({
            "export": [
                "project_export { projectId } needs export_data. It hands over the app folder: folder, a download of konae.json, app/ and dist/, and a manifest with one download per part of data/: database, identity, files and, when the app recorded any, usage. sourcesAhead compares app/ with the release's sourceDigest. Older releases without a digest compare app/actions.json with the actions the release runs. Secrets, owners and the installation's records never leave.",
                "Read each download with transfer_read { transferId, offset } until nextOffset equals size, check its SHA-256, then transfer_cancel it.",
                "bun run konae -- export --dir <folder> --out <new folder> --with-data writes the same app folder on disk."
            ],
            "withoutKonae": [
                "The kernel binary serves an exported folder with its data/: the folder's compose.yaml runs the image Dockerfile.kernel builds, named by KONAE_KERNEL_IMAGE in .env. At startup it checks the folder as a release and prints the app's address.",
                "There the kernel binary's commands do what the owner tools do, with the same refusals: users, invite, suspend, resume, signup-policy, api-key, secret, sql, events, maintenance, enter and import. The folder's AGENTS.md lists them; under Compose run docker compose exec kernel kernel <command>."
            ],
            "import": [
                "On the destination Kernel, project_create with the same projectId, and secret_set for every secret the app's routes sign with, and, when the app offers Google sign-in, google_client_id and google_client_secret with use google. Register the destination's redirect URIs, which modules_read lists, in that Google client.",
                "For each part of the data the manifest lists, transfer_begin { size, sha256, projectId, component } with its name, transfer_write each block and transfer_complete.",
                "Build the folder, upload the release JSON of the publishing topic with initialPackage { manifest, components: { <name>: <transferId> } }, then release_plan and release_apply.",
                "Sessions, email links and live API keys stay behind: people sign in with their passwords, and api_keys_list shows each key revoked at import so the owner issues new ones."
            ],
            "importExisting": [
                "project_import { projectId, replace, transferId?, sourcesTransferId?, artifactTransferId?, package?, confirmSchema? } replaces an existing app with an exported folder and needs publish. replace is sources (publish the folder's app/ and dist/), data (the database, accounts, files and Usage) or both. It never merges rows.",
                "For sources, upload the release JSON built from the folder and pass transferId. Large app/ files can travel in a separate sourcesTransferId upload, and a large dist/ artifact can travel in artifactTransferId. For data, upload each component as a new app's import does and pass package { manifest, components }. The manifest lists each component's size and SHA-256.",
                "It writes a backup of what it replaces first. database_execute list-backups lists it with holds data and or sources, and restore-backup brings it back after backing up the current state again.",
                "The app keeps its Signup Policy, sign-in methods, addresses, secrets, and the sessions and API keys it has now, for every account the new data holds.",
                "When the schema change loses data the answer is { imported: false, schema } and nothing changed. Show the owner each loss and call again with schema.confirmation as confirmSchema.",
                "It works while the app is in maintenance, so a move with no lost write is maintenance_set on, project_export, project_import on the destination, maintenance_set off."
            ],
        }),
        "refusals" => refusals(),
        _ => bail!("Unknown guide topic. Choose one of {}.", TOPICS.join(", ")),
    })
}

fn refusals() -> Value {
    let mut groups = serde_json::Map::new();
    for (code, row) in super::refusal::table() {
        let group = code.split('.').next().unwrap_or(code);
        let entry = groups.entry(group.to_owned()).or_insert_with(|| json!({}));
        entry[code.as_str()] = row.clone();
    }
    json!({
        "shape": "A refused Capsule answers { code, message, hint }. An MCP tool puts it under structuredContent.error and keeps the sentence in content[0].text; the editor's Validation Gate gives the same code and sentence.",
        "fix": {
            "capsule": "An edit to the app's files clears it. Change the file the message names and check again.",
            "owner": "The owner clears it outside the files, with the tool the hint names, such as secret_set. No edit clears it.",
            "operator": "Only the installation can clear it."
        },
        "rule": "Branch on code, never on message. A code never changes once released; a sentence may.",
        "codes": groups
    })
}

pub(super) fn session_can(owner: &Owner) -> Value {
    let holds = |scope: &str| owner.scopes.iter().any(|s| s == scope);
    json!({
        "read": holds("read"),
        "write": holds("write"),
        "publish": holds("publish"),
        "manage_users": holds("manage_users"),
        "export_data": holds("export_data"),
        "createTokens": owner.session
    })
}

fn login_flag(scope: &str) -> Option<&'static str> {
    match scope {
        "manage_users" => Some("--manage-users"),
        "export_data" => Some("--export-data"),
        _ => None,
    }
}

/// Why `owner` may not call `name`, for a tool the administrative endpoint
/// would list with other permissions. None for a name no credential reaches.
pub(super) fn missing_permission(name: &str, owner: &Owner) -> Option<String> {
    let lists = |probe: &Owner| admin_tools(probe).iter().any(|tool| tool["name"] == name);
    for scope in SCOPES {
        let mut probe = owner.clone();
        probe.session = false;
        probe.scopes = vec![scope.to_owned()];
        if lists(&probe) {
            let how = match login_flag(scope) {
                Some(flag) => format!("Run konae login with {flag}, or use a token from token_create that includes it."),
                None => "Run konae login, or use a token from token_create that includes it.".to_owned(),
            };
            return Some(format!("{name} needs a credential with {scope}. {how}"));
        }
    }
    let mut session = owner.clone();
    session.session = true;
    session.scopes = SCOPES.iter().map(|scope| (*scope).to_owned()).collect();
    if lists(&session) {
        return Some(format!("{name} needs the owner's browser session. A delegated token cannot create or list tokens."));
    }
    if runtime_tools().iter().any(|tool| tool["name"] == name) {
        return Some(format!("{name} runs on an app's runtime endpoint, /_konae/mcp on the app's host, with an app session."));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(scopes: &[&str]) -> Owner {
        Owner { id: "o".into(), display: None, scopes: scopes.iter().map(|s| (*s).to_owned()).collect(), projects: vec![], session: false }
    }

    #[test]
    fn a_refused_tool_names_the_permission_and_the_login_flag() {
        let publisher = owner(&["read", "write", "publish"]);
        assert_eq!(
            missing_permission("app_invitations_create", &publisher).unwrap(),
            "app_invitations_create needs a credential with manage_users. Run konae login with --manage-users, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("project_export", &publisher).unwrap(),
            "project_export needs a credential with export_data. Run konae login with --export-data, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("release_plan", &owner(&["read"])).unwrap(),
            "release_plan needs a credential with publish. Run konae login, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("token_create", &publisher).unwrap(),
            "token_create needs the owner's browser session. A delegated token cannot create or list tokens."
        );
        assert_eq!(
            missing_permission("actions_invoke", &publisher).unwrap(),
            "actions_invoke runs on an app's runtime endpoint, /_konae/mcp on the app's host, with an app session."
        );
        assert_eq!(missing_permission("no_such_tool", &publisher), None);
    }

    /// Every tool name a topic mentions is one the Kernel serves, so the
    /// guide cannot send an agent to a tool that was renamed or removed.
    #[test]
    fn every_tool_a_topic_names_exists() {
        let mut session = owner(&SCOPES);
        session.session = true;
        let served: Vec<String> = admin_tools(&session)
            .into_iter()
            .chain(runtime_tools())
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        let prefixes = ["platform_", "project_", "projects_", "database_", "release_", "signup_policy_", "sign_in_methods_", "secret_", "secrets_", "api_keys_", "usage_", "signals_", "calls_", "notices_", "contact_email_", "hostname_", "maintenance_", "app_user", "app_invitation", "transfer_", "token_", "tokens_", "auth_", "actions_", "files_", "mail_", "session_", "domain_", "path_"];
        for name in TOPICS {
            let text = topic(name).unwrap().to_string();
            for word in text.split(|c: char| !(c.is_ascii_lowercase() || c == '_')) {
                if word.contains('_') && prefixes.iter().any(|prefix| word.starts_with(prefix)) && word != "app_invitations" {
                    assert!(served.contains(&word.to_owned()), "the {name} topic names {word}, which the Kernel does not serve");
                }
            }
        }
        assert!(topic("nothing").unwrap_err().to_string().starts_with("Unknown guide topic."));
    }

    #[test]
    fn every_write_tool_names_a_read_back_or_a_gap() {
        let mut session = owner(&SCOPES);
        session.session = true;
        let writes: Vec<String> = admin_tools(&session)
            .into_iter()
            .chain(runtime_tools())
            .filter(|tool| tool["annotations"]["readOnlyHint"] == false)
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        let served: Vec<String> = admin_tools(&session)
            .into_iter()
            .chain(runtime_tools())
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        let mapped: Vec<&str> = READ_BACK
            .iter()
            .chain(READ_BACK_IN_ANSWER)
            .chain(READ_BACK_GAPS)
            .map(|(w, _)| *w)
            .collect();
        for write in &writes {
            assert_eq!(mapped.iter().filter(|w| *w == write).count(), 1, "write tool {write} needs exactly one read-back, answer or gap");
        }
        for (write, _) in READ_BACK_IN_ANSWER {
            assert!(writes.iter().any(|s| s == *write), "inAnswer names {write}, which is not a write tool");
        }
        for (write, _) in READ_BACK_EXPECT {
            assert!(READ_BACK.iter().any(|(w, _)| w == write), "expect names {write}, which has no read-back tool");
        }
        for (write, read) in READ_BACK {
            assert!(served.iter().any(|s| s == *write), "read-back map names {write}, which the Kernel does not serve");
            assert!(served.iter().any(|s| s == *read), "read-back for {write} names {read}, which the Kernel does not serve");
        }
        for (write, _) in READ_BACK_GAPS {
            assert!(served.iter().any(|s| s == *write), "gap names {write}, which the Kernel does not serve");
            assert!(!READ_BACK.iter().any(|(w, _)| w == write), "gap {write} also has a read-back");
        }
    }

    /// The refusals topic is the table, so an agent reads the remedy for a
    /// code through the same door that refused it.
    #[test]
    fn the_refusals_topic_lists_every_code_with_its_fix() {
        let topic = topic("refusals").unwrap();
        let mut listed = 0;
        for (group, codes) in topic["codes"].as_object().unwrap() {
            for (code, row) in codes.as_object().unwrap() {
                assert!(code.starts_with(&format!("{group}.")));
                assert_eq!(row, &super::super::refusal::table()[code]);
                listed += 1;
            }
        }
        assert_eq!(listed, super::super::refusal::table().len());
        assert_eq!(topic["codes"]["route"]["route.signature-secret-unset"]["fix"], "owner");
    }

    #[test]
    fn rules_and_pieces_topics_carry_the_operating_contract() {
        let rules = topic("rules").unwrap();
        assert!(rules["yardstick"].as_str().unwrap().contains("take it to another machine"));
        assert_eq!(rules["firstRun"]["mcp"].as_array().unwrap().len(), 3);
        // The first run is the three steps of AGENTS.md: an app that runs, a credential, a proof.
        // Uploading by transfer belongs to the publishing topic, after the agent has an app.
        for step in rules["firstRun"]["mcp"].as_array().unwrap() {
            assert!(!step.as_str().unwrap().contains("transfer_"), "{step}");
        }
        let actions = topic("actions").unwrap();
        let actions_rules = actions["rules"].as_str().unwrap();
        assert!(!actions_rules.contains("id'}}"), "{actions_rules}");
        assert_eq!(actions_rules.matches("to answer on that path").count(), 1);
        assert_eq!(rules["readBack"]["map"]["release_apply"], "project_inspect");
        // A block's answer carries nextOffset and a plan's answer is the plan,
        // so neither sends the agent to a second call.
        for write in ["transfer_write", "release_plan"] {
            assert!(rules["readBack"]["map"].get(write).is_none(), "{write}");
            assert!(rules["readBack"]["inAnswer"][write].is_string(), "{write}");
        }
        assert!(rules["readBack"]["expect"]["transfer_cancel"].as_str().unwrap().contains("Transfer not found or expired."));
        assert!(rules["divergences"].as_array().unwrap().len() >= 3);
        let pieces = topic("pieces").unwrap();
        assert!(pieces["choose"].as_array().unwrap().len() >= 8);
        assert!(pieces["unit"].as_str().unwrap().contains("app folder"));
    }

    /// A coding agent reads AGENTS.md and an MCP agent reads these topics,
    /// so both get the same table of pieces and the same divergences.
    #[test]
    fn pieces_and_divergences_are_the_ones_agents_md_names() {
        let agents = include_str!("../../../AGENTS.md");
        let section = |heading: &str| agents.split(heading).nth(1).unwrap().split("\n## ").next().unwrap().to_owned();
        let rows: Vec<Vec<String>> = section("## Pick the smallest piece")
            .lines()
            .filter(|line| line.starts_with("| ") && !line.starts_with("| Need") && !line.starts_with("| ---"))
            .map(|line| line.trim_matches('|').split(" | ").map(|cell| cell.trim().replace('`', "")).collect())
            .collect();
        let choose: Vec<Vec<String>> = topic("pieces").unwrap()["choose"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| ["need", "use", "not"].iter().map(|key| row[*key].as_str().unwrap().to_owned()).collect())
            .collect();
        assert_eq!(choose, rows);
        let divergences = section("## Named divergences").lines().filter(|line| line.starts_with("- ")).count();
        assert_eq!(topic("rules").unwrap()["divergences"].as_array().unwrap().len(), divergences);
    }

    #[test]
    fn session_can_follows_scopes_and_browser_session() {
        let delegated = owner(&["read", "publish"]);
        assert_eq!(session_can(&delegated)["publish"], true);
        assert_eq!(session_can(&delegated)["write"], false);
        assert_eq!(session_can(&delegated)["createTokens"], false);
        let mut browser = owner(&SCOPES);
        browser.session = true;
        assert_eq!(session_can(&browser)["createTokens"], true);
        assert_eq!(session_can(&browser)["export_data"], true);
    }
}
