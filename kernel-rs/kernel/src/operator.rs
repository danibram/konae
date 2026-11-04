//! The commands an operator runs on an app that a Kernel serves without
//! Konae (ADR-0027, decision 3). Each one calls the function Konae's owner
//! tool calls, so it refuses with the same words, and prints the same answer
//! as JSON. They work on the app's files beside a running Kernel: the app
//! accounts, keys and secrets are SQLite files the Kernel reads on every
//! request, and the Kernel follows `konae.json` for the Signup Policy.

use std::io::Read;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::installation::Installation;
use crate::{
    admin_database, find_project_deployment, http, invitations, layout, persist_deployment, runtime, AppState, Deployment,
    Settings, SignupMode, SignupPolicy, Startup,
};

const COMMANDS: &[&str] = &["users", "invite", "suspend", "resume", "signup-policy", "api-key", "secret", "sql", "events"];

pub fn usage() -> &'static str {
    "Commands on an app this Kernel serves. Add --app <id> when the data directory holds more than one app.
  kernel users [--state pending|active|suspended] [--cursor <email>]
  kernel invite <email>...
  kernel suspend <user id>
  kernel resume <user id>
  kernel signup-policy open|invitation|closed|allowlist [<email or @domain>...]
  kernel api-key list
  kernel api-key create <name> [--action <name>]... [--files public|private]...
  kernel api-key revoke <key id>
  kernel secret list
  kernel secret set <name> [--use route|call|google]   reads the value from standard input
  kernel sql <statement>
  kernel events --from <YYYY-MM-DD> --to <YYYY-MM-DD>"
}

/// The key that seals queued links when `KONAE_MAIL_SECRET` is unset. The
/// Kernel and its commands read the same one.
pub fn mail_key() -> String {
    std::env::var("KONAE_ADMIN_KEY").ok().filter(|key| !key.is_empty()).unwrap_or_else(|| crate::DEFAULT_ADMIN_KEY.into())
}

/// Runs the command the process was started with, and answers false when
/// it names none of these.
pub async fn command(installation: &Installation) -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("help" | "--help")) {
        println!("{}", usage());
        return Ok(true);
    }
    let Some(name) = args.first().filter(|name| COMMANDS.contains(&name.as_str())) else {
        return Ok(false);
    };
    let mut words = Arguments::new(&args[1..]);
    let app = words.option("app")?;
    let state = crate::state_from_env(&Startup::from_env()?, installation.clone(), &mail_key(), Vec::new(), None).await?.0;
    let id = app_id(&state, app).await?;
    let answer = run(&state, &id, name, &mut words).await?;
    println!("{}", serde_json::to_string_pretty(&answer)?);
    Ok(true)
}

async fn run(state: &AppState, id: &str, name: &str, words: &mut Arguments) -> Result<Value> {
    if name == "secret" {
        let deployment = find_project_deployment(&*state.deployments.read().await, id);
        let db = http::secrets_store(&state.data_root, id)?;
        return match words.next().as_deref() {
            Some("list") => http::secrets_call(&db, id, deployment.as_ref(), "secrets_list", &json!({})),
            Some("set") => {
                let usage = words.option("use")?;
                let secret = words.required("the secret's name")?;
                let mut value = String::new();
                std::io::stdin().read_to_string(&mut value).context("Could not read the value from standard input")?;
                let mut arguments = json!({"name": secret, "value": value.trim_end_matches(['\r', '\n'])});
                if let Some(usage) = usage {
                    arguments["use"] = json!(usage);
                }
                http::secrets_call(&db, id, deployment.as_ref(), "secret_set", &arguments)
            }
            _ => bail!("{}", usage()),
        };
    }
    let deployment = served(state, id).await?;
    match name {
        "users" => {
            let mut arguments = json!({});
            for key in ["state", "cursor"] {
                if let Some(value) = words.option(key)? {
                    arguments[key] = json!(value);
                }
            }
            invitations::run(state, &deployment, &operator(state, id), "app_users_list", &arguments).await
        }
        "invite" => {
            let emails = words.rest();
            ensure!(!emails.is_empty(), "Name at least one email to invite.");
            let arguments = json!({"emails": emails, "idempotencyKey": Uuid::new_v4().to_string()});
            invitations::run(state, &deployment, &operator(state, id), "app_invitations_create", &arguments).await
        }
        "suspend" | "resume" => {
            let user = words.required("the user's ID, as kernel users lists it")?;
            let tool = if name == "suspend" { "app_user_suspend" } else { "app_user_resume" };
            invitations::run(state, &deployment, &operator(state, id), tool, &json!({"userId": user})).await
        }
        "signup-policy" => signup_policy(state, deployment, words).await,
        "api-key" => {
            let (tool, arguments) = match words.next().as_deref() {
                Some("list") => ("api_keys_list", json!({})),
                Some("create") => {
                    let actions = words.options("action")?;
                    let files = words.options("files")?;
                    ("api_keys_create", json!({"name": words.required("the key's name")?, "actions": actions, "files": files}))
                }
                Some("revoke") => ("api_keys_revoke", json!({"keyId": words.required("the key's ID")?})),
                _ => bail!("{}", usage()),
            };
            http::api_keys_call(state, &deployment, tool, &arguments).await
        }
        "sql" => {
            let sql = words.rest().join(" ");
            ensure!(!sql.trim().is_empty(), "Give the statement in quotes.");
            let body = json!({"operation": "sql", "sql": sql});
            runtime::response_value(admin_database(state, id, body.to_string().as_bytes()).await).await
        }
        "events" => {
            let (from, to) = (words.required_option("from")?, words.required_option("to")?);
            let range = json!({"from": from, "to": to});
            Ok(json!({
                "summary": crate::usage::admin_call(&state.data_root, &deployment, "usage_summary", &range).await?,
                "calls": crate::calls::admin_call(state, &deployment, &json!({})).await?["calls"],
                "notices": crate::mail::notices_tool(state, &deployment, &json!({})).await?["notices"],
                "signals": crate::usage::admin_call(&state.data_root, &deployment, "signals_read", &json!({})).await?["signals"],
            }))
        }
        _ => bail!("{}", usage()),
    }
}

/// `signup_policy_set` as Konae's tool runs it: the entries stay when none
/// are given, and the change lands in `konae.json`, which the running
/// Kernel follows.
async fn signup_policy(state: &AppState, deployment: Deployment, words: &mut Arguments) -> Result<Value> {
    let mode: SignupMode = serde_json::from_value(json!(words.required("open, invitation, closed or allowlist")?))
        .map_err(|_| anyhow!("The Signup Policy is open, invitation, closed or allowlist."))?;
    let given = words.rest();
    let entries = if given.is_empty() { deployment.signup_policy.allowlist.clone() } else { given };
    let policy = SignupPolicy::from_input(mode, &entries).map_err(anyhow::Error::msg)?;
    let _guard = crate::database_lock(state, &format!("project:{}", deployment.project_id)).await;
    let mut updated = Settings::read(&state.data_root, &deployment.id)?.deployment(
        crate::Installed {
            hostname: deployment.hostname.clone(),
            created_at: deployment.created_at,
            actions: deployment.actions.clone(),
            artifact_version: deployment.artifact_version.clone(),
        },
        &state.public_origin,
    );
    let previous = updated.signup_policy.mode;
    updated.signup_policy = policy;
    persist_deployment(state, &updated).await?;
    let visibility = if crate::is_private(&updated) { "private" } else { "public" };
    Ok(json!({"deploymentId": updated.id, "visibility": visibility, "mode": updated.signup_policy.mode, "allowlist": updated.signup_policy.allowlist, "previousMode": previous}))
}

/// Who sends an invitation from the machine: the app's contact email.
fn operator(state: &AppState, id: &str) -> invitations::Inviter {
    let email = Settings::read(&state.data_root, id).ok().and_then(|settings| settings.contact_email);
    invitations::Inviter { id: "operator".into(), email }
}

async fn served(state: &AppState, id: &str) -> Result<Deployment> {
    find_project_deployment(&*state.deployments.read().await, id)
        .ok_or_else(|| anyhow!("apps/{id} has not been served yet. Start the Kernel on it once, then run the command again."))
}

/// The app a command is for: `--app`, or the only folder under `apps/`.
async fn app_id(state: &AppState, app: Option<String>) -> Result<String> {
    if let Some(id) = app {
        ensure!(layout::settings(&state.data_root, &id).exists(), "{} holds no app {id}.", layout::apps(&state.data_root).display());
        return Ok(id);
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(layout::apps(&state.data_root))? {
        let entry = entry?;
        if entry.path().join(layout::SETTINGS_FILE).exists() {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    match ids.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!("{} holds no app folder.", layout::apps(&state.data_root).display()),
        _ => {
            ids.sort();
            bail!("Name the app with --app <id>: {}.", ids.join(", "))
        }
    }
}

/// The words after the command, read in order, with `--name value` options taken out.
struct Arguments {
    words: Vec<String>,
}

impl Arguments {
    fn new(words: &[String]) -> Self {
        Self { words: words.to_vec() }
    }

    fn options(&mut self, name: &str) -> Result<Vec<String>> {
        let flag = format!("--{name}");
        let mut values = Vec::new();
        while let Some(index) = self.words.iter().position(|word| *word == flag) {
            ensure!(index + 1 < self.words.len() && !self.words[index + 1].starts_with("--"), "{flag} needs a value.");
            values.push(self.words.remove(index + 1));
            self.words.remove(index);
        }
        Ok(values)
    }

    fn option(&mut self, name: &str) -> Result<Option<String>> {
        let mut values = self.options(name)?;
        ensure!(values.len() <= 1, "Give --{name} once.");
        Ok(values.pop())
    }

    fn required_option(&mut self, name: &str) -> Result<String> {
        self.option(name)?.ok_or_else(|| anyhow!("Give --{name}.\n{}", usage()))
    }

    fn next(&mut self) -> Option<String> {
        (!self.words.is_empty()).then(|| self.words.remove(0))
    }

    fn required(&mut self, what: &str) -> Result<String> {
        self.next().ok_or_else(|| anyhow!("Name {what}.\n{}", usage()))
    }

    fn rest(&mut self) -> Vec<String> {
        std::mem::take(&mut self.words)
    }
}
