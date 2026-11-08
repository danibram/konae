//! What `platform_guide` answers, and the hint a refused tool call carries.
//! docs/mcp.md is the long form of every topic; docs/mcp-walkthrough.md runs
//! them in order against a Kernel.

use super::mcp::{admin_tools, Owner};
use super::runtime::runtime_tools;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// The topics `platform_guide` takes, in the order an agent meets them.
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

/// The permissions a delegated credential may hold. The owner's browser
/// session holds all of them.
const SCOPES: [&str; 5] = ["read", "write", "publish", "manage_users", "export_data"];

/// How to observe the effect of a write through the same door. A write with
/// no entry here must be listed in [`READ_BACK_GAPS`]. Staging transfers are
/// outside the rule until `transfer_complete` or a publish that consumes them.
pub(super) const READ_BACK: &[(&str, &str)] = &[
    ("project_create", "project_inspect"),
    ("project_sources_save", "project_sources_read"),
    ("project_delete", "projects_list"),
    ("database_execute", "database_execute"),
    ("secret_set", "secrets_list"),
    ("secret_delete", "secrets_list"),
    ("api_keys_create", "api_keys_list"),
    ("api_keys_revoke", "api_keys_list"),
    ("release_plan", "release_plan"),
    ("release_apply", "project_inspect"),
    ("signup_policy_set", "signup_policy_read"),
    ("sign_in_methods_set", "sign_in_methods_read"),
    ("app_invitations_create", "app_invitations_list"),
    ("app_invitation_resend", "app_invitations_list"),
    ("app_invitation_revoke", "app_invitations_list"),
    ("app_user_suspend", "app_users_list"),
    ("app_user_resume", "app_users_list"),
    ("app_user_enter", "auth_session"),
    ("domain_set", "domain_read"),
    ("domain_clear", "domain_read"),
    ("path_set", "path_read"),
    ("path_clear", "path_read"),
    ("maintenance_set", "maintenance_read"),
    ("contact_email_set", "contact_email_read"),
    ("transfer_begin", "transfer_status"),
    ("transfer_write", "transfer_status"),
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

/// Writes whose effect this door cannot re-read yet. Each entry names what
/// would close the gap. Kept empty on purpose when every write has a read-back.
pub(super) const READ_BACK_GAPS: &[(&str, &str)] = &[];

pub(super) fn topic(name: &str) -> Result<Value> {
    Ok(match name {
        "rules" => json!({
            "yardstick": "Konae lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.",
            "decide": "A new capability belongs in the product only when the Validation Gate and the Kernel refuse it with the same words, and an export of the app folder can carry the app's accounts, data and rules. Anything else is operator work, not an App feature.",
            "sameChecks": "A query that reads rows names a LIMIT in the editor and on the Kernel that leaves with the app. Who may open the app, and who may read a row, stay decisions of the app.",
            "firstRun": {
                "cli": [
                    "bun run konae -- login --dir <folder> --url <Kernel URL>",
                    "bun run konae -- init --dir <folder> --name '<name>' && bun run konae -- dev --dir <folder>",
                    "bun run konae -- publish --dir <folder>, then open the Deployment URL and call one Server Action"
                ],
                "mcp": [
                    "A credential: bun run konae -- login, then platform_guide topics rules, pieces and credentials",
                    "An app that runs: bun run konae -- init, then dev or publish; the publishing topic has the same steps as MCP calls",
                    "Proof: project_inspect for the Deployment URL, open it, call one Server Action with actions_invoke, and conform against action-contract.json when checking a contract"
                ],
                "after": "If the first run grows past these steps, the product got harder. Prefer fixing the path over adding a guide page."
            },
            "readBack": {
                "rule": "Every tool that writes has a tool that observes its effect through the same door. After a write, call that read-back before retrying or telling the owner it worked. A service_timeout or interrupted call must be verified the same way.",
                "map": Value::Object(READ_BACK.iter().map(|(write, read)| ((*write).to_owned(), json!(*read))).collect()),
                "gaps": Value::Object(READ_BACK_GAPS.iter().map(|(write, close)| ((*write).to_owned(), json!(*close))).collect()),
                "staging": "Bytes in an open transfer are not a Deployment. transfer_status is the read-back of staging; project_inspect is the read-back of a release that consumed them."
            },
            "capabilities": "session_inspect answers can for the scopes this credential holds, and createTokens when it is the owner's browser session. Decide from can, not from memory of which flag login took.",
            "divergences": [
                {"where": "editor preview vs published Deployment", "fact": "The preview applies no Content Security Policy. Inline event handlers and other-origin fetches can still run there; a published app refuses them. Attach listeners in JavaScript and keep scripts and fetches on the app's own origin."},
                {"where": "editor preview vs Deployment", "fact": "The preview has no HTTP routes, no schedule timer, no notice email, no Usage store and no call outbox. Try those on bun run konae -- dev or on a published Deployment."},
                {"where": "editor preview vs Deployment", "fact": "The editor preview has no Google sign-in. miniapp.auth.signInWithGoogle() refuses there; try it on a published Deployment once the owner has set the app's Google client and turned Google on."},
                {"where": "Validation Gate vs publish builders", "fact": "The Gate checks schema and actions.json. A page that names a missing file fails at build or release, not at validate_capsule. Check every path you write."}
            ]
        }),
        "pieces" => json!({
            "rule": "Pick the smallest piece that does the job. Do not invent a second process, a second database door or a host rule the Capsule can already declare.",
            "choose": [
                {"need": "Show a page and keep rows the app owns", "use": "Capsule files plus Server Actions in actions.json (query or mutation)", "not": "Raw SQL from the browser, or a Database API outside Server Actions"},
                {"need": "Let a visitor or member change or read a row", "use": "A named Server Action with access and actor bindings", "not": "Owner SQL from database_execute in the app UI"},
                {"need": "Refuse a mutation that matched no row", "use": "guard on that mutation", "not": "A second check statement or a platform error invent"},
                {"need": "Email the owner, the caller or the people a changed row names", "use": "notify on that mutation", "not": "A Call Action to a mail API from the browser"},
                {"need": "Work that must run when nobody is looking", "use": "schedule on a mutation with no input", "not": "An external cron hitting a public route unless a human asked for that door"},
                {"need": "Record that something happened for the owner", "use": "signal on a mutation, or kind signal", "not": "miniapp.track or a side table only the agent reads"},
                {"need": "Another service calls this app", "use": "http on an anonymous action with callers, signature and/or API key", "not": "A Call Action (that is outbound)"},
                {"need": "This app calls another app or https service", "use": "kind call with url, body, optional auth and then", "not": "fetch of a secret from the browser"},
                {"need": "Files a person uploads", "use": "miniapp.files", "not": "Base64 in a SQLite column for large bytes"},
                {"need": "Who may create an app account", "use": "Signup Policy and invitations", "not": "A homemade users table that bypasses Runtime Actor"},
                {"need": "A short link on the installation's domain", "use": "path address (path_set)", "not": "A custom domain when DNS is not yours"},
                {"need": "Take the app to another machine", "use": "export and import of the app folder", "not": "Copy (that moves the whole installation)"},
                {"need": "Debug, or try the next version, on what the published app holds", "use": "konae pull (an export of the data), then dev --reset (its import)", "not": "Publishing to find out"},
                {"need": "See the email, calls, scheduled runs and Signals of a local run", "use": "konae events (--json for an agent), or the page dev serves", "not": "Reading the Kernel's files by hand"}
            ],
            "unit": "A Deployment folder on the Kernel is the app: artifact, database, identity, files and Usage. Prefer adding a declaration inside the Capsule over adding a new platform resource."
        }),
        "credentials" => json!({
            "login": "bun run konae -- login --dir <folder> --url <Kernel URL> reads KONAE_EMAIL and KONAE_PASSWORD. Add KONAE_INVITATION_CODE, the code the operator issued with konae creator-code-create, to create the account, or KONAE_SETUP_KEY for an installation's first owner. It saves a 24-hour token with read, write and publish in <folder>/.konae/credentials.json, which publish, pull, sql and export read. KONAE_MCP_TOKEN overrides that file. On a Kernel that verifies email, creating the account sends a link; on a development Kernel on the same machine, --verify-from <data folder>/mailbox.jsonl answers it with KONAE_PASSWORD in the same run.",
            "flags": {"--manage-users": "adds manage_users", "--export-data": "adds export_data"},
            "oauth": "An MCP client with OAuth reads /.well-known/oauth-authorization-server, registers at /oauth/register, and sends the owner to /oauth/authorize with PKCE S256, resource <origin>/mcp and a scope of read, write, publish and manage_users. The token lasts one hour and there is no refresh token. OAuth does not offer export_data.",
            "permissions": {
                "read": "projects_list, project_inspect, signup_policy_read, sign_in_methods_read, maintenance_read, secrets_list, api_keys_list, usage_read, signals_read, calls_read, notices_read",
                "write": "project_create, project_sources_save, project_delete and database_execute changes",
                "publish": "release_plan, release_apply, secret_set, secret_delete, api_keys_create, api_keys_revoke, maintenance_set, project_import, and initialSignupPolicy on a first release",
                "manage_users": "signup_policy_set, sign_in_methods_set, app_users_list, app_user_suspend, app_user_resume, app_user_enter from the owner's browser session, and the app_invitations tools",
                "export_data": "project_export"
            },
            "refusals": "A tool this credential may not call answers 'Unknown tool or insufficient permission.' and names the permission it needs. A delegated token cannot create tokens or add permissions to itself, so run login again with the flag. session_inspect lists scopes and can for what the current credential holds."
        }),
        "development" => json!({
            "commands": [
                "bun run konae -- init --dir /tmp/my-app --name 'My app' [--template tasks|notes|members|form|webhook|digest|gallery]",
                "bun run konae -- dev --dir /tmp/my-app [--reset]",
                "bun run konae -- build --dir /tmp/my-app",
                "bun run konae -- contract --dir /tmp/my-app",
                "bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 [--export-data]",
                "bun run konae -- publish --dir /tmp/my-app",
                "bun run konae -- pull --dir /tmp/my-app",
                "bun run konae -- sql --dir /tmp/my-app [--prod] '<statement>'",
                "bun run konae -- events --dir /tmp/my-app [--follow] [--json] [--prod]",
                "bun run konae -- conform --url <app URL> --contract /tmp/my-app/action-contract.json --token <app session> --key <API key> --secret <name>=<value>"
            ],
            "credentials": "init, dev, build and contract, and sql and events without --prod, need no credential. login saves the credential and the installation's address in the folder, and publish, pull and sql read it from there or from KONAE_MCP_TOKEN. The first publish registers the project with project_create. The credentials topic lists the permissions. Never put a token in app source.",
            "liveData": "pull is an export of the published app's data (database, accounts, files, Usage) into .konae/pull/data and needs export_data. dev --reset imports it into the folder's data/ with the kernel binary's import command, after a backup in .konae/backups. Nothing travels back to the published app. The Kernel brings data/ to app/schema.sql when it starts, so a column the next version adds is tried on the published rows first. sql and events read the folder's data/ through the kernel binary's commands, and the published Deployment only with --prod.",
            "events": "events lists what the local app did besides answer its page: email with its recipient and text (nothing is sent; it lands in .konae/kernel/mailbox.jsonl), calls to other services, scheduled runs, routes and Signals. dev serves the same list as a page on the port after the app's. Run events with --json after calling an action to check a notify, a call or a signal did what the Capsule declares, and with --follow to watch.",
            "start": "In an empty folder, either run init, or call project_create with a name and an optional template and write the files it returns into app/ plus konae.json {id, name, slug, visibility, signupPolicy, signInMethods, kernel, manifest}. Both give the same files. init also writes AGENTS.md, action-contract.md, compose.yaml, .env.example and a package.json whose npm run build writes dist/. Then dev runs it locally and publish sends it to the installation.",
            "isolation": "dev runs the kernel binary on the folder with its own data/, no owner and no MCP, and does not proxy production. It keeps the Signup Policy konae.json names; on an app that is not Open, invite a test account with the kernel binary's invite command, as dev prints it.",
            "sources": "publish sends the text files of app/ and the built dist/. The editor saves its sources into the app folder's app/ on the installation with project_sources_save, so a publish from a laptop's folder does not change what the editor shows.",
            "contract": "contract writes action-contract.json, its schema, action-contract.md and actions.d.ts, which types miniapp.actions.call, from actions.json and schema.sql. conform calls every action on a Deployment and checks the answers against that file, so point it at a test Deployment. The owner reads what ran with usage_read and signals_read."
        }),
        "actions" => json!({"filename":"actions.json","format":{"version":1,"actions":{"items.list":{"kind":"query","access":"authenticated","statement":"SELECT id,title FROM items WHERE user_id=?1 LIMIT 50","bindings":[{"source":"actor","field":"id"}]},"items.create":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO items(title,user_id) VALUES(?1,?2)","bindings":[{"source":"input","key":"title","type":"string","maxLength":200},{"source":"actor","field":"id"}]}}},"rules":"SQL and typed bindings stay server-side. Declare schema in schema.sql for artifact publication. Runtime calls only send the declared action name and input. Add nullable:true to an input binding to bind omitted values or explicit null as SQL NULL; supplied strings remain non-empty and all supplied values retain their type checks. Mutations return changes and lastInsertId, so read rows with a query instead of mutation RETURNING, which publication rejects. A query that reads rows needs a top-level LIMIT. A total with count, sum, avg, min or max and no GROUP BY does not. An anonymous action may add http:{method:'GET' for a query or 'POST' for a mutation,path,signature?:{scheme?:'konae'|'hmac-body'|'timestamped'|'svix',header,secret,...},fields?:{inputKey:'$.data.object.id'},callers?:['signature'|'key'|'session']} to answer on that path of its Deployment host; a provider's scheme and fields take a webhook as GitHub, Shopify, Stripe or Resend send it; an action whose route does not answer anyone answers only there, not through actions_invoke. The sdk topic's http module describes routes, callers, signatures, API keys and secret_set. A mutation may add guard:'<sentence>' so a statement that changes no row is refused with it instead of answering changes 0. A mutation may add notify:{to:'owner' or 'actor' or {column:'<name>'},changes?:['<column>'],subject,body} to queue email when it changes a row: to the app's contact email in konae.json, which the owner sets in App settings or with contact_email_set and a first publish from Konae fills with the owner's email when Konae knows it, to the calling account once it has proved its email, or to each proven account of this app whose ID a changed row holds in that column, with only its own rows. The templates take {input.<key>} for its input bindings, {actor.id}, {kernel.now}, {changes.count} and, in the body, {changes}, which lists the changed rows' changes columns; it never sends to an address from the input or a row. Usage counts each notice under notify, and the owner reads each queued notice's state with notices_read. A mutation may add signal:{name:'order.placed',properties:[input keys or 'actor']} to record a Signal when it changes a row, and kind:'signal' with access, bindings and signal but no statement records one without SQL. The owner reads them with signals_read and Usage with usage_read. kind:'call' with access, input bindings only, method:'GET'|'POST', url, body:{bodyKey:'inputKey' or {value:constant}}, auth?:{secret,header?}|{key}, then?:{action,inputs:{key:'/status'|'/body/...'|'/input/...'}} queues a request the Kernel sends from its own process and answers {callId,queued:true}; the calls module of the sdk topic gives the address rules, and the owner reads the outbox with calls_read."}),
        // Rendered from src/sdk/contract.ts by `bun run sdk:render`.
        "sdk" => serde_json::from_str::<Value>(include_str!("sdk-guide.json"))?,
        "publishing" => json!({
            "steps": [
                "Create project identity with project_create. Keep its id, slug and manifest locally.",
                "Set every secret an action's http signature names with secret_set { projectId, name, value }. A release that signs with an unset secret is refused and names each one.",
                "Build static HTML/CSS/JS locally. Inject the SDK using the local builder.",
                "Upload JSON {project:{id,name,slug,files,manifest},visibility,artifact:{version:1,entrypoint:'index.html',files:{...}}} using transfer_begin with projectId, byte size and sha256, then transfer_write and transfer_complete.",
                "Call release_plan with transferId, inspect its planId, expectedRevision and schema, then call release_apply with planId.",
                "schema is the change the Kernel derived from schema.sql to the live database, with statements, losses and confirmation. A new table, column or index applies. When losses is not empty (a dropped table, a dropped column, a changed column type), show the owner each loss's sentence and pass confirmation as confirmSchema only after the owner agrees. Without it release_apply refuses with schema.confirmation-required and changes nothing.",
                "Save the receipt and cancel the temporary transfer. Read the live Deployment with project_inspect."
            ],
            "cli": "bun run konae -- publish --dir <folder> runs these steps with the credential from login and sends signupPolicy from konae.json as initialSignupPolicy on the first release.",
            "concurrency": "An obsolete plan is rejected. Repeating an applied plan returns the same receipt.",
            "access": "A first release may add initialSignupPolicy {mode, allowlist} with mode open, invitation, closed or allowlist; the Kernel installs it before the app serves. When the owner's request leaves access open for an app with accounts, propose visibility private and mode invitation. Later changes need manage_users and signup_policy_set. Invite people with app_invitations_create only when the owner asks, because it sends email.",
            "import": "A first release may also carry initialPackage to start the Deployment with an exported app's data. project_import replaces an existing app's sources, data or both; the portability topic has the steps.",
            "domain": "domain_set { deploymentId, hostname } adds a custom domain next to the generated hostname and needs publish. Its first answer names a TXT record, _konae.<hostname>, and the token it must hold. Once the record is at the DNS provider and the hostname points at the installation's proxy, which terminates TLS and forwards Host unchanged, call domain_set again: the Kernel looks the record up and the alias serves. domain_read shows the state and domain_clear removes it. bun run konae -- domain runs the same steps.",
            "maintenance": "maintenance_set { deploymentId, on } puts the app in maintenance or takes it out and needs publish. Paused, the app serves a maintenance page, refuses writes, signups and sign-ins, and holds its email, calls and schedule; owner tools that would change it are refused except maintenance_set and project_import. maintenance_read { deploymentId } answers { maintenance, since, unreadable, installation }. Move an app with no lost write by putting it in maintenance, exporting it, and importing the folder elsewhere.",
            "path": "path_set { deploymentId, enabled, name?, acceptSharedOrigin? } answers the app on <domain>/p/<id>, and on <domain>/p/<name> once it has a name. The domain is KONAE_DOMAIN. An app with accounts or a private app needs acceptSharedOrigin true, because every path and the home share one origin, and any other app on the domain can use the sessions of this app's users. The app whose enabled path holds the name konae-home also answers <domain>/ and every path there outside /p/; with none, <domain>/ answers not found. path_read returns the state and the addresses; path_clear turns the path off and removes the name. Both need publish. Read back with path_read after a change."
        }),
        "users" => json!({
            "permission": "manage_users, which login adds with --manage-users. signup_policy_read and sign_in_methods_read need only read.",
            "policy": "signup_policy_set { deploymentId, mode } with open, invitation, closed or allowlist and allowlist entries. A publish never changes it after the first release.",
            "google": "Each app has its own Google OAuth client, which the owner makes in their own Google console. sign_in_methods_read { deploymentId } returns redirectUris, the callbacks to register in that client: the app's own host, a verified custom domain, and <domain>/p/<id> while the path address is on (the name address uses the id's callback; the home address offers no Google), and googleClient { clientIdSet, clientSecretSet }. secret_set { projectId, name: \"google_client_id\" or \"google_client_secret\", value, use: \"google\" } sets each half; the value is never returned and no route or call reads it. Then sign_in_methods_set { deploymentId, methods: [\"password\", \"google\"] } adds Continue with Google to the sign-in page; [\"password\"] removes it. Password cannot be turned off. While either half is unset it refuses with \"Set this app's Google client ID and client secret before turning Google on.\", and while Google is on secret_delete refuses either half. A Google sign-in reaches the app account that holds the same email, made with a password or with Google, and Signup Policy decides whether an email without an account gets one. Google proves only an address it manages: a gmail.com address, or one whose Workspace domain the token names in hd; any other address is refused and signs in with email and password. An active account whose email nobody proved is taken back for the person Google vouches for: its password and sessions stop working. A package carries signInMethods and the accounts, never the client: on another installation the owner sets the two secrets and registers that host's redirect URI, and until then the sign-in page offers only email and password and sign_in_methods_read returns a notice.",
            "invite": [
                "Call mail_status: inviting needs sending true.",
                "app_invitations_create { deploymentId, emails, idempotencyKey } emails each address a seven-day link to /_konae/invitation#invitation=<token> on the app's host. Call it only when the owner asked for those invitations.",
                "The person opens the link and chooses a password; the page posts it to /_konae/auth/accept and signs them in.",
                "app_invitations_list shows each invitation's status; app_invitation_resend sends a new link and app_invitation_revoke ends one."
            ],
            "accounts": "app_users_list, app_user_suspend and app_user_resume. Suspension ends sessions and keeps the account's rows.",
            "enter": "app_user_enter { deploymentId, userId } answers a link on the app's own address that works once, for two minutes, and signs the browser that opens it in as that active account. It needs manage_users and the owner's own browser session, so an agent cannot call it; the editor's Users view has the button. The audit log records who entered as whom, and Usage and Signals do not count what that session does.",
            "roles": "An app account has no role the Kernel keeps. An app that needs an administrator keeps roles in its own table, such as profiles(user_id, role), checks them in SQL with the actor binding { source: actor, field: id }, and the owner names the first one with database_execute sql. auth_session answers role member for every signed-in account and anonymous for a visitor."
        }),
        "portability" => json!({
            "export": [
                "project_export { deploymentId } needs export_data. It hands over the app folder: folder, a download of konae.json, app/ and dist/, and a manifest with one download per part of data/: database, identity, files and, when the app recorded any, usage. sourcesAhead is true when app/actions.json differs from the actions the release runs. Secrets, owners and the installation's records never leave.",
                "Read each download with transfer_read { transferId, offset } until nextOffset equals size, check its SHA-256, then transfer_cancel it.",
                "bun run konae -- export --dir <folder> --out <new folder> --with-data writes the same app folder on disk."
            ],
            "withoutKonae": [
                "The kernel binary serves an exported folder with its data/: the folder's compose.yaml runs the image Dockerfile.kernel builds, named by KONAE_KERNEL_IMAGE in .env. At startup it checks the folder as a release and prints the app's address.",
                "There the kernel binary's commands do what the owner tools do, with the same refusals: users, invite, suspend, resume, signup-policy, api-key, secret, sql, events, maintenance, enter and import. The folder's AGENTS.md lists them; under Compose run docker compose exec kernel kernel <command>."
            ],
            "import": [
                "On the destination Kernel, project_create with the same projectId, and secret_set for every secret the app's routes sign with, and, when the app offers Google sign-in, google_client_id and google_client_secret with use google. Register the destination's redirect URIs, which sign_in_methods_read lists, in that Google client.",
                "For each part of the data the manifest lists, transfer_begin { size, sha256, projectId, component } with its name, transfer_write each block and transfer_complete.",
                "Build the folder, upload the release JSON of the publishing topic with initialPackage { manifest, components: { <name>: <transferId> } }, then release_plan and release_apply.",
                "Sessions, email links and live API keys stay behind: people sign in with their passwords, and api_keys_list shows each key revoked at import so the owner issues new ones."
            ],
            "importExisting": [
                "project_import { deploymentId, replace, transferId?, package?, confirmSchema? } replaces an existing app with an exported folder and needs publish. replace is sources (publish the folder's app/ and dist/), data (the database, accounts, files and Usage) or both. It never merges rows.",
                "For sources, upload the release JSON of the publishing topic built from the folder and pass its transferId. For data, upload each component as a new app's import does and pass package { manifest, components }. The manifest lists each component's size and SHA-256.",
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

/// Every code a refused Capsule answers, rendered from refusal-codes.json
/// (ADR-0004), grouped by the part of the Capsule it names.
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

/// What `session_inspect` reports under `can`: the scopes this credential
/// holds, plus whether it may create tokens (owner browser session only).
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

/// The flag `konae login` takes to add a permission, when it has one.
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
        let prefixes = ["platform_", "project_", "projects_", "database_", "storage_", "release_", "signup_policy_", "sign_in_methods_", "secret_", "secrets_", "api_keys_", "usage_", "signals_", "calls_", "notices_", "app_user", "app_invitation", "transfer_", "token_", "tokens_", "auth_", "actions_", "files_", "mail_", "session_", "domain_", "path_"];
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
        let mapped: Vec<&str> = READ_BACK.iter().map(|(w, _)| *w).chain(READ_BACK_GAPS.iter().map(|(w, _)| *w)).collect();
        for write in &writes {
            assert!(mapped.contains(&write.as_str()), "write tool {write} has no read-back and no gap");
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
        // The first run is the three steps of AGENTS.md: a credential, an app that runs, a proof.
        // Uploading by transfer belongs to the publishing topic, after the agent has an app.
        for step in rules["firstRun"]["mcp"].as_array().unwrap() {
            assert!(!step.as_str().unwrap().contains("transfer_"), "{step}");
        }
        let actions = topic("actions").unwrap();
        let actions_rules = actions["rules"].as_str().unwrap();
        assert!(!actions_rules.contains("id'}}"), "{actions_rules}");
        assert_eq!(actions_rules.matches("to answer on that path").count(), 1);
        assert_eq!(rules["readBack"]["map"]["release_apply"], "project_inspect");
        assert!(rules["divergences"].as_array().unwrap().len() >= 3);
        let pieces = topic("pieces").unwrap();
        assert!(pieces["choose"].as_array().unwrap().len() >= 8);
        assert!(pieces["unit"].as_str().unwrap().contains("Deployment folder"));
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
