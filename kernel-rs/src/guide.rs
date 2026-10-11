//! What `platform_guide` answers, and the hint a refused tool call carries.
//! docs/mcp.md is the long form of every topic; docs/mcp-walkthrough.md runs
//! them in order against a Kernel.

use super::mcp::{admin_tools, runtime_tools, Owner};
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// The topics `platform_guide` takes, in the order an agent meets them.
pub(super) const TOPICS: [&str; 7] = ["credentials", "development", "actions", "sdk", "publishing", "users", "portability"];

/// The permissions a delegated credential may hold. The owner's browser
/// session holds all of them.
const SCOPES: [&str; 6] = ["read", "write", "publish", "manage_users", "export_data", "manage_lifecycle"];

pub(super) fn topic(name: &str) -> Result<Value> {
    Ok(match name {
        "credentials" => json!({
            "login": "bun run littlebox -- login --dir <folder> --url <Kernel URL> reads LITTLEBOX_EMAIL and LITTLEBOX_PASSWORD. Add LITTLEBOX_INVITATION_CODE, the code the operator issued with littlebox-kernel creator-code-create, to create the account, or LITTLEBOX_SETUP_KEY for an installation's first owner. It saves a 24-hour token with read, write and publish in <folder>/.littlebox/credentials.json, which init, publish, export and eject read. LITTLEBOX_MCP_TOKEN overrides that file. On a Kernel that verifies email, creating the account sends a link; on a development Kernel on the same machine, --verify-from <data folder>/mailbox.jsonl answers it with LITTLEBOX_PASSWORD in the same run.",
            "flags": {"--manage-users": "adds manage_users", "--export-data": "adds export_data", "--manage-lifecycle": "adds manage_lifecycle"},
            "oauth": "An MCP client with OAuth reads /.well-known/oauth-authorization-server, registers at /oauth/register, and sends the owner to /oauth/authorize with PKCE S256, resource <origin>/mcp and a scope of read, write, publish and manage_users. The token lasts one hour and there is no refresh token. OAuth does not offer export_data or manage_lifecycle.",
            "permissions": {
                "read": "projects_list, project_inspect, signup_policy_read, secrets_list, api_keys_list, usage_read, signals_read, calls_read, migration_status",
                "write": "project_create, project_sources_save, schema_plan, and database_execute changes",
                "publish": "release_plan, release_apply, secret_set, secret_delete, api_keys_create, api_keys_revoke, and initialSignupPolicy on a first release",
                "manage_users": "signup_policy_set, app_users_list, app_user_suspend, app_user_resume and the app_invitations tools",
                "export_data": "package_export, project_export, database_export, and migration_begin together with manage_lifecycle",
                "manage_lifecycle": "eject_plan, eject_apply and the migration tools"
            },
            "refusals": "A tool this credential may not call answers 'Unknown tool or insufficient permission.' and names the permission it needs. A delegated token cannot create tokens or add permissions to itself, so run login again with the flag. session_inspect lists what the current credential holds."
        }),
        "development" => json!({
            "commands": [
                "bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173",
                "bun run littlebox -- init --dir /tmp/my-app --name 'My app' --url http://localhost:4173",
                "bun run littlebox -- dev --dir /tmp/my-app",
                "bun run littlebox -- build --dir /tmp/my-app",
                "bun run littlebox -- contract --dir /tmp/my-app",
                "bun run littlebox -- publish --dir /tmp/my-app",
                "bun run littlebox -- conform --url <app URL> --contract /tmp/my-app/action-contract.json --token <app session> --key <API key> --secret <name>=<value>"
            ],
            "credentials": "login saves the credential in the folder, and init and publish read it from there or from LITTLEBOX_MCP_TOKEN. The credentials topic lists the permissions. Never put a token in app source.",
            "isolation": "dev uses the same Rust Kernel with separate local identities, data and files under .littlebox/dev-data, publishes there as Open so test accounts can sign up, and does not proxy production.",
            "sourceAuthority": "Code and builds stay in your folder. Source is not synchronized to the browser editor.",
            "contract": "contract writes action-contract.json, its schema and action-contract.md from actions.json and schema.sql. conform calls every action on a Deployment and checks the answers against that file, so point it at a test Deployment. The owner reads what ran with usage_read and signals_read."
        }),
        "actions" => json!({"filename":"actions.json","format":{"version":1,"actions":{"items.list":{"kind":"query","access":"authenticated","statement":"SELECT id,title FROM items WHERE user_id=?1 LIMIT 50","bindings":[{"source":"actor","field":"id"}]},"items.create":{"kind":"mutation","access":"authenticated","statement":"INSERT INTO items(title,user_id) VALUES(?1,?2)","bindings":[{"source":"input","key":"title","type":"string","maxLength":200},{"source":"actor","field":"id"}]}}},"rules":"SQL and typed bindings stay server-side. Declare schema in schema.sql for artifact publication. Runtime calls only send the declared action name and input. Add nullable:true to an input binding to bind omitted values or explicit null as SQL NULL; supplied strings remain non-empty and all supplied values retain their type checks. Mutations return changes and lastInsertId, so read rows with a query instead of mutation RETURNING, which publication rejects. A query that reads rows needs a top-level LIMIT. A total with count, sum, avg, min or max and no GROUP BY does not. An anonymous action may add http:{method:'GET' for a query or 'POST' for a mutation,path,signature?:{scheme?:'littlebox'|'hmac-body'|'timestamped'|'svix',header,secret,...},fields?:{inputKey:'$.data.object.id'},callers?:['signature'|'key'|'session']} to answer on that path of its Deployment host; an action whose route does not answer anyone answers only there, not through actions_invoke. The sdk topic's http module describes routes, callers, signatures, API keys and secret_set. A mutation may add guard:'<sentence>' so a statement that changes no row is refused with it instead of answering changes 0. A mutation may add notify:{to:'owner' or 'actor',subject,body} to queue one email when it changes a row: to the Project Owner, or to the calling account once it has proved its email. The templates take {input.<key>} for its input bindings and {actor.id}, nothing else; it never sends to an address from the input. Usage counts each notice under notify. A mutation may add signal:{name:'order.placed',properties:[input keys or 'actor']} to record a Signal when it changes a row, and kind:'signal' with access, bindings and signal but no statement records one without SQL. The owner reads them with signals_read and Usage with usage_read. id'}} to answer on that path of its Deployment host; a provider's scheme and fields take a webhook as GitHub, Shopify, Stripe or Resend send it; an action whose route does not answer anyone answers only there, not through actions_invoke. kind:'call' with access, input bindings only, method:'GET'|'POST', url, body:{bodyKey:'inputKey' or {value:constant}}, auth?:{secret,header?}|{key}, then?:{action,inputs:{key:'/status'|'/body/...'|'/input/...'}} queues a request the Kernel sends from its own process and answers {callId,queued:true}; the calls module of the sdk topic gives the address rules, and the owner reads the outbox with calls_read."}),
        // Rendered from src/sdk/contract.ts by `bun run sdk:render`.
        "sdk" => serde_json::from_str::<Value>(include_str!("sdk-guide.json"))?,
        "publishing" => json!({
            "steps": [
                "Create project identity with project_create. Keep its id, slug and manifest locally.",
                "Set every secret an action's http signature names with secret_set { projectId, name, value }. A release that signs with an unset secret is refused and names each one.",
                "Build static HTML/CSS/JS locally. Inject the SDK using the local builder.",
                "Upload JSON {project:{id,name,slug,files,manifest},visibility,artifact:{version:1,entrypoint:'index.html',files:{...}}} using transfer_begin with projectId, byte size and sha256, then transfer_write and transfer_complete.",
                "Call release_plan with transferId, inspect its planId and expectedRevision, then call release_apply with planId.",
                "Save the receipt and cancel the temporary transfer."
            ],
            "cli": "bun run littlebox -- publish --dir <folder> runs these steps with the credential from login and sends project.manifest.signupPolicy from littlebox.json as initialSignupPolicy on the first release.",
            "concurrency": "An obsolete plan is rejected. Repeating an applied plan returns the same receipt.",
            "access": "A first release may add initialSignupPolicy {mode, allowlist} with mode open, invitation, closed or allowlist; the Kernel installs it before the app serves. When the owner's request leaves access open for an app with accounts, propose visibility private and mode invitation. Later changes need manage_users and signup_policy_set. Invite people with app_invitations_create only when the owner asks, because it sends email.",
            "import": "A first release may also carry initialPackage to start the Deployment with an exported app's data; the portability topic has the steps.",
            "domain": "domain_set { deploymentId, hostname } adds a custom domain next to the generated hostname and needs publish. Its first answer names a TXT record, _littlebox.<hostname>, and the token it must hold. Once the record is at the DNS provider and the hostname points at the installation's proxy, which terminates TLS and forwards Host unchanged, call domain_set again: the Kernel looks the record up and the alias serves. domain_read shows the state and domain_clear removes it. bun run littlebox -- domain runs the same steps."
        }),
        "users" => json!({
            "permission": "manage_users, which login adds with --manage-users. signup_policy_read needs only read.",
            "policy": "signup_policy_set { deploymentId, mode } with open, invitation, closed or allowlist and allowlist entries. A publish never changes it after the first release.",
            "invite": [
                "Call mail_status: inviting needs sending true.",
                "app_invitations_create { deploymentId, emails, idempotencyKey } emails each address a seven-day link to /_littlebox/invitation#invitation=<token> on the app's host. Call it only when the owner asked for those invitations.",
                "The person opens the link and chooses a password; the page posts it to /_littlebox/auth/accept and signs them in.",
                "app_invitations_list shows each invitation's status; app_invitation_resend sends a new link and app_invitation_revoke ends one."
            ],
            "accounts": "app_users_list, app_user_suspend and app_user_resume. Suspension ends sessions and keeps the account's rows.",
            "owner": "The Project Owner's own app account has the role owner, which auth_session returns and the binding { source: actor, field: role } yields. app_owner_enter { deploymentId }, for the owner's browser session only, creates or links it and returns a link that signs it in once within two minutes; an invitation the owner sends to their own email gives the same role. Guard owner-only actions with ?n = 'owner' on that binding instead of a key-only route."
        }),
        "portability" => json!({
            "export": [
                "package_export { deploymentId } needs export_data. It returns a manifest and one download per component: database, identity, files and, when the app recorded any, usage.",
                "Read each download with transfer_read { transferId, offset } until nextOffset equals size, check its SHA-256, then transfer_cancel it.",
                "bun run littlebox -- export --dir <folder> --with-data writes the same package into a self-hosted kit."
            ],
            "import": [
                "On the destination Kernel, project_create with the same projectId, and secret_set for every secret the app's routes sign with.",
                "For each component, transfer_begin { size, sha256, projectId, component } with the component's name, transfer_write each block and transfer_complete.",
                "Build the folder, upload the release JSON of the publishing topic with initialPackage { manifest, components: { <name>: <transferId> } }, then release_plan and release_apply.",
                "Sessions, email links and live API keys stay behind: people sign in with their passwords, and api_keys_list shows each key revoked at import so the owner issues new ones."
            ],
            "eject": [
                "Eject moves a Managed project, one the editor saves with project_sources_save, to its owner's folder. A project made with littlebox init is already External.",
                "bun run littlebox -- eject --dir <empty folder> --project <id> --url <Kernel URL> calls eject_plan and eject_apply, which need manage_lifecycle, and writes the sources and a littlebox.json.",
                "bun run littlebox -- publish --dir <that folder> publishes from it. The Deployment, its URL and its data stay."
            ],
            "migration": [
                "migration_begin { deploymentId } needs manage_lifecycle and export_data. The app pauses at once and the answer holds the package downloads and next steps.",
                "Import on the destination as above, adding the release download as the release payload, then migration_confirm here and migration_activate there.",
                "migration_cancel { operationId } before migration_confirm resumes the app. migration_status reads the state."
            ]
        }),
        _ => bail!("Unknown guide topic. Choose one of {}.", TOPICS.join(", ")),
    })
}

/// The flag `littlebox login` takes to add a permission, when it has one.
fn login_flag(scope: &str) -> Option<&'static str> {
    match scope {
        "manage_users" => Some("--manage-users"),
        "export_data" => Some("--export-data"),
        "manage_lifecycle" => Some("--manage-lifecycle"),
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
                Some(flag) => format!("Run littlebox login with {flag}, or use a token from token_create that includes it."),
                None => "Run littlebox login, or use a token from token_create that includes it.".to_owned(),
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
        return Some(format!("{name} runs on an app's runtime endpoint, /_littlebox/mcp on the app's host, with an app session."));
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
            "app_invitations_create needs a credential with manage_users. Run littlebox login with --manage-users, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("package_export", &publisher).unwrap(),
            "package_export needs a credential with export_data. Run littlebox login with --export-data, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("eject_plan", &publisher).unwrap(),
            "eject_plan needs a credential with manage_lifecycle. Run littlebox login with --manage-lifecycle, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("release_plan", &owner(&["read"])).unwrap(),
            "release_plan needs a credential with publish. Run littlebox login, or use a token from token_create that includes it."
        );
        assert_eq!(
            missing_permission("token_create", &publisher).unwrap(),
            "token_create needs the owner's browser session. A delegated token cannot create or list tokens."
        );
        assert_eq!(
            missing_permission("actions_invoke", &publisher).unwrap(),
            "actions_invoke runs on an app's runtime endpoint, /_littlebox/mcp on the app's host, with an app session."
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
        let prefixes = ["platform_", "project_", "projects_", "schema_", "database_", "storage_", "release_", "signup_policy_", "secret_", "secrets_", "api_keys_", "usage_", "signals_", "calls_", "app_user", "app_invitation", "transfer_", "token_", "tokens_", "auth_", "actions_", "files_", "eject_", "migration_", "package_", "mail_", "session_"];
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
}
