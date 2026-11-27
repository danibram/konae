use std::io::Read;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::installation::Installation;
use crate::{
    AppState, Deployment, Settings, SignupMode, Startup, admin_database, copies, file_store,
    find_project_deployment, http, invitations, layout, recovery, runtime,
};

const COMMANDS: &[&str] = &["users", "invite", "suspend", "resume", "signup-policy", "module", "modules", "api-key", "secret", "sql", "events", "import", "maintenance", "enter", "test", "queues", "ftps"];

pub fn usage() -> &'static str {
    "Commands on an app this Kernel serves. Add --app <id> when the data directory holds more than one app.
  kernel users [--state pending|active|suspended] [--cursor <email>]
  kernel invite <email>...
  kernel suspend <user id>
  kernel resume <user id>
  kernel signup-policy open|invitation|closed|allowlist [<email or @domain>...]
  kernel modules
  kernel module <name> on|off
  kernel api-key list
  kernel api-key create <name> [--action <name>]... [--files public|private]...
  kernel api-key revoke <key id>
  kernel secret list
  kernel secret set <name> [--use route|call|google]   reads the value from standard input
  kernel sql <statement>
  kernel events --from <YYYY-MM-DD> --to <YYYY-MM-DD>
  kernel capabilities
  kernel inventory [--policy]   inspect the installation or print its store policies
  kernel schema fingerprint --schema <schema.sql> | --database <database.sqlite>
  kernel test status [--app <id>]
  kernel test schedule <action|cron:id> [--app <id>]
  kernel test calls [--app <id>]
  kernel test intake <source> --path <relative-path> --file <completed-file> [--delivery-key <key>] [--app <id>]
  kernel test reply <call id> [--app <id>]   reads response JSON from standard input
  kernel test reset-effects [--app <id>]   retires pending work after a stopped local reset
  kernel queues preflight|migrate|rollback <backup>   migrate legacy queues while consumers are stopped
  kernel ftps status   show the installation's saved FTPS listener settings
  kernel maintenance on|off
  kernel enter <email>   prints a one-time link that signs this browser in as the account, for 2 minutes
  kernel import <folder> [--confirm-schema <confirmation>]   replaces the app's data with the data/ of an exported folder, after a backup"
}

/// The file in the data directory that holds the key the Kernel alone seals
/// queued links with.
pub const MAIL_KEY_FILE: &str = "mail-key";

/// The key that seals queued links when `KONAE_MAIL_SECRET` is unset. A
/// Kernel without Konae has no setup key, and the development default is
/// public, so the first start writes a random key into the data directory,
/// readable by its owner alone. The Kernel and its commands read the same file.
pub fn mail_key(data_root: &std::path::Path) -> Result<String> {
    use std::io::Write;
    let path = data_root.join(MAIL_KEY_FILE);
    std::fs::create_dir_all(data_root)?;
    let key = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(key.as_bytes())?;
            file.sync_all()?;
            Ok(key)
        }
        Err(error_value) if error_value.kind() == std::io::ErrorKind::AlreadyExists => {
            let kept = std::fs::read_to_string(&path)
                .with_context(|| format!("{} cannot be read", path.display()))?;
            ensure!(
                kept.len() >= 32,
                "{} holds no key. Remove it, and the next start writes a new one; queued links sealed with the old key stop working.",
                path.display()
            );
            Ok(kept)
        }
        Err(error_value) => {
            Err(anyhow::Error::from(error_value)
                .context(format!("Could not write {}", path.display())))
        }
    }
}

/// Runs the command the process was started with, and answers false when
/// it names none of these.
pub async fn command(installation: &Installation) -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|name| name == "capabilities") {
        ensure!(args.len() == 1, "kernel capabilities takes no arguments.");
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::execution::capabilities())?
        );
        return Ok(true);
    }
    if args.first().is_some_and(|name| name == "schema") {
        let mut words = Arguments::new(&args[1..]);
        ensure!(
            words.next().as_deref() == Some("fingerprint"),
            "{}",
            usage()
        );
        let schema = words.option("schema")?;
        let database = words.option("database")?;
        ensure!(words.rest().is_empty(), "{}", usage());
        let fingerprint = schema_fingerprint(schema.as_deref(), database.as_deref())?;
        println!("{}", json!({"fingerprint": fingerprint}));
        return Ok(true);
    }
    if matches!(args.first().map(String::as_str), Some("help" | "--help")) {
        println!("{}", usage());
        return Ok(true);
    }
    let Some(name) = args
        .first()
        .filter(|name| COMMANDS.contains(&name.as_str()))
    else {
        return Ok(false);
    };
    if name == "ftps" {
        let mut words = Arguments::new(&args[1..]);
        ensure!(words.next().as_deref() == Some("status") && words.rest().is_empty(), "kernel ftps status\n{}", usage());
        let startup = Startup::from_env()?;
        println!("{}", serde_json::to_string_pretty(&crate::ftps::configured_status(&startup.data_root)?)?);
        return Ok(true);
    }
    let mut words = Arguments::new(&args[1..]);
    let app = words.option("app")?;
    let startup = Startup::from_env()?;
    let _stopped = if name == "queues" {
        Some(crate::file_store::hold_while_stopped(&startup.data_root, "queue migration")?)
    } else { None };
    if name == "queues" {
        ensure!(std::env::var("KONAE_QUEUE_CONSUMERS_PAUSED").as_deref() == Ok("1"), "Stop the Kernel service and set KONAE_QUEUE_CONSUMERS_PAUSED=1 for this offline queue operation.");
        let deployments = crate::installation::queue_migration::deployments(&startup.data_root, &startup.hosts.public_origin)?;
        let answer = queues_command(&startup.data_root, &deployments, &mut words)?;
        println!("{}", serde_json::to_string_pretty(&answer)?);
        return Ok(true);
    }
    let _operation = crate::file_store::external_operation(&startup.data_root)?;
    let _generation = if name == "import" {
        Some(crate::file_store::external_operation(&layout::apps(
            &startup.data_root,
        ))?)
    } else {
        None
    };
    let state = crate::state_from_env(
        &startup,
        installation.clone(),
        &mail_key(&startup.data_root)?,
        Vec::new(),
        None,
    )
    .await?
    .0;
    let id = app_id(&state, app).await?;
    let answer = if name == "test" {
        test_command(&state, &id, &mut words).await
    } else {
        run(&state, &id, name, &mut words).await
    };
    ensure!(
        crate::usage::flush_status(),
        "Could not drain operator Usage writes."
    );
    println!("{}", serde_json::to_string_pretty(&answer?)?);
    Ok(true)
}

fn schema_fingerprint(schema: Option<&str>, database: Option<&str>) -> Result<String> {
    match (schema, database) {
        (Some(path), None) => {
            let sql =
                std::fs::read_to_string(path).with_context(|| format!("Cannot read {path}"))?;
            Ok(crate::schema_transform::fingerprint_schema(&sql)?)
        }
        (None, Some(path)) => {
            let connection = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .with_context(|| format!("Cannot read {path}"))?;
            Ok(crate::schema_transform::fingerprint(&connection)?)
        }
        _ => bail!("Choose exactly one of --schema <schema.sql> or --database <database.sqlite>."),
    }
}

async fn test_command(state: &AppState, id: &str, words: &mut Arguments) -> Result<Value> {
    ensure!(
        state.execution_mode.is_test(),
        "Kernel test controls require KONAE_EXECUTION_MODE=test."
    );
    match words.next().as_deref() {
        Some("status") => {
            ensure!(words.rest().is_empty(), "{}", usage());
            let deployment = served(state, id).await?;
            Ok(
                json!({"mode": "test", "pausedSchedules": crate::schedule::paused_schedules(&deployment)}),
            )
        }
        Some("schedule") => {
            let action = words.required("a scheduled action name or cron:id")?;
            ensure!(words.rest().is_empty(), "{}", usage());
            let deployment = served(state, id).await?;
            crate::schedule::run_manually(state, &deployment, &action).await
        }
        Some("calls") => {
            ensure!(words.rest().is_empty(), "{}", usage());
            Ok(json!({"calls": crate::calls::list_test_calls_for_app(state, id).await?}))
        }
        Some("intake") => {
            let source = words.required("a declared source ID")?;
            let relative_path = words.required_option("path")?;
            let staged = words.required_option("file")?;
            let delivery_key = words.option("delivery-key")?;
            ensure!(words.rest().is_empty(), "{}", usage());
            let deployment = served(state, id).await?;
            ensure!(crate::modules::is_on(&deployment.modules, "file-intake"), "Turn on the file-intake module before accepting files.");
            let accepted = crate::file_intake::accept_staged(state, &deployment, &source, std::path::Path::new(&staged), &relative_path, delivery_key.as_deref()).await?;
            Ok(serde_json::to_value(accepted)?)
        }
        Some("reply") => {
            let call = words.required("the captured call's ID")?;
            ensure!(words.rest().is_empty(), "{}", usage());
            let (number, outcome) = read_test_reply(std::io::stdin().lock())?;
            let result =
                crate::calls::submit_synthetic_response(state, id, &call, number, outcome).await?;
            Ok(
                json!({"accepted": result == crate::calls::SyntheticSubmission::Accepted, "callId": call, "responseNumber": number}),
            )
        }
        Some("reset-effects") => {
            ensure!(words.rest().is_empty(), "{}", usage());
            let calls = crate::calls::retire_after_local_reset(state, id).await?;
            let root = state.data_root.clone();
            let app = id.to_owned();
            let mail = tokio::task::spawn_blocking(move || -> Result<usize> {
                let test = crate::mail::end_pending_for_local_reset(
                    &root.join(crate::mail::TEST_MAIL_DATABASE),
                    &app,
                )?;
                let legacy = crate::mail::end_pending_for_local_reset(
                    &root.join(crate::mail::MAIL_DATABASE),
                    &app,
                )?;
                Ok(test + legacy)
            })
            .await??;
            Ok(json!({"callsRetired": calls, "mailRetired": mail}))
        }
        _ => bail!("{}", usage()),
    }
}

fn queues_command(data_root: &std::path::Path, deployments: &[Deployment], words: &mut Arguments) -> Result<Value> {
    let operation = words.required("preflight, migrate or rollback")?;
    match operation.as_str() {
        "preflight" | "migrate" => {
            ensure!(words.rest().is_empty(), "kernel queues {operation}");
            let report = if operation == "migrate" {
                crate::installation::queue_migration::migrate(data_root, deployments)?
            } else {
                crate::installation::queue_migration::preflight(data_root, deployments)?
            };
            Ok(serde_json::to_value(report)?)
        }
        "rollback" => {
            let backup = words.required("the backup path returned by queue migration")?;
            ensure!(words.rest().is_empty(), "kernel queues rollback <backup>");
            crate::installation::queue_migration::rollback(data_root, std::path::Path::new(&backup))?;
            Ok(json!({"rolledBack": true, "backup": backup}))
        }
        _ => anyhow::bail!("kernel queues preflight|migrate|rollback <backup>"),
    }
}

fn read_test_reply(reader: impl Read) -> Result<(i64, crate::calls::SyntheticOutcome)> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Reply {
        Http(HttpReply),
        Network(NetworkReply),
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct HttpReply {
        response_number: i64,
        status: u16,
        body: Value,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct NetworkReply {
        response_number: i64,
        error: String,
    }

    let mut bytes = Vec::new();
    reader
        .take(65_537)
        .read_to_end(&mut bytes)
        .context("Could not read response JSON from standard input")?;
    ensure!(bytes.len() <= 65_536, "Synthetic response exceeds 64 KiB.");
    let reply: Reply = serde_json::from_slice(&bytes).context("Reply JSON needs a positive responseNumber and either status (100 to 599) with body, or error.")?;
    let (number, outcome) = match reply {
        Reply::Http(reply) => {
            ensure!(
                (100..=599).contains(&reply.status),
                "HTTP status must be between 100 and 599."
            );
            let body = match reply.body {
                Value::String(text) => text.into_bytes(),
                value => serde_json::to_vec(&value)?,
            };
            (
                reply.response_number,
                crate::calls::SyntheticOutcome::Http {
                    status: reply.status,
                    body,
                },
            )
        }
        Reply::Network(reply) => {
            ensure!(!reply.error.is_empty(), "Reply error must not be empty.");
            (
                reply.response_number,
                crate::calls::SyntheticOutcome::Network { error: reply.error },
            )
        }
    };
    ensure!(number > 0, "Reply JSON needs a positive responseNumber.");
    Ok((number, outcome))
}

async fn run(state: &AppState, id: &str, name: &str, words: &mut Arguments) -> Result<Value> {
    if name == "secret" {
        let deployment = find_project_deployment(&*state.deployments.read().await, id);
        return match words.next().as_deref() {
            Some("list") => http::secrets_call(
                &http::read_secrets(&state.data_root, id)?,
                id,
                deployment.as_ref(),
                "secrets_list",
                &json!({}),
            ),
            Some("set") => {
                let usage = words.option("use")?;
                let secret = words.required("the secret's name")?;
                let mut value = String::new();
                std::io::stdin()
                    .read_to_string(&mut value)
                    .context("Could not read the value from standard input")?;
                let mut arguments =
                    json!({"name": secret, "value": value.trim_end_matches(['\r', '\n'])});
                if let Some(usage) = usage {
                    arguments["use"] = json!(usage);
                }
                http::secrets_call(
                    &http::secrets_store(&state.data_root, id)?,
                    id,
                    deployment.as_ref(),
                    "secret_set",
                    &arguments,
                )
            }
            _ => bail!("{}", usage()),
        };
    }
    if name == "import" {
        return import(state, id, words).await;
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
            invitations::run(
                state,
                &deployment,
                &operator(state, id),
                "app_users_list",
                &arguments,
            )
            .await
        }
        "invite" => {
            let emails = words.rest();
            ensure!(!emails.is_empty(), "Name at least one email to invite.");
            // A serving Kernel checks its sender at startup; this process checks it before it queues.
            if let Some(queue) = state.accounts.queue.as_ref() {
                queue.run_preflight().await;
            }
            let arguments = json!({"emails": emails, "idempotencyKey": Uuid::new_v4().to_string()});
            invitations::run(
                state,
                &deployment,
                &operator(state, id),
                "app_invitations_create",
                &arguments,
            )
            .await
        }
        "suspend" | "resume" => {
            let user = words.required("the user's ID, as kernel users lists it")?;
            let tool = if name == "suspend" {
                "app_user_suspend"
            } else {
                "app_user_resume"
            };
            invitations::run(
                state,
                &deployment,
                &operator(state, id),
                tool,
                &json!({"userId": user}),
            )
            .await
        }
        "signup-policy" => signup_policy(state, deployment, words).await,
        "modules" => {
            ensure!(
                words.rest().is_empty(),
                "kernel modules takes no arguments."
            );
            Ok(crate::modules::read(state, &deployment))
        }
        "module" => {
            let name = words
                .next()
                .context("kernel module needs a module name and on or off.")?;
            let value = words.next().context("kernel module needs on or off.")?;
            ensure!(
                words.rest().is_empty() && ["on", "off"].contains(&value.as_str()),
                "kernel module <name> on|off"
            );
            let _release_guard = crate::named_lock(state, &format!("publish:{}", deployment.id), true).await;
            let result = crate::modules::set(
                &state.data_root,
                &deployment.id,
                &name,
                Some(value == "on"),
                None,
                None,
            )
            .await?;
            let mut deployments = state.deployments.write().await;
            if let Some(current) = deployments
                .values_mut()
                .find(|current| current.id == deployment.id)
            {
                current.modules.insert(name, result["state"].clone());
            }
            Ok(result)
        }
        "maintenance" => {
            let on = match words.required("on or off")?.as_str() {
                "on" => true,
                "off" => false,
                _ => bail!("Say maintenance on or maintenance off."),
            };
            let result = crate::maintenance::set(&state.data_root, id, on)?;
            if !on {
                crate::modules::wake_durable_work(state, id).await;
            }
            Ok(result)
        }
        "enter" => {
            let email = words.required("the account's email")?;
            let user = account_with_email(state, id, &email)?;
            // The person at the machine is the operator; their login name, when
            // the shell has one, says which one.
            let who = std::env::var("USER")
                .ok()
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| "operator".to_owned(), |name| format!("operator:{name}"));
            let link = crate::enter::issue(state, &deployment, &user, &who).await?;
            let account = link["email"].as_str().unwrap_or(&email);
            eprintln!(
                "{who} may enter app {id} as {account} with this link, once, within 2 minutes. The serving Kernel logs the entry."
            );
            Ok(link)
        }
        "api-key" => {
            let (tool, arguments) = match words.next().as_deref() {
                Some("list") => ("api_keys_list", json!({})),
                Some("create") => {
                    let actions = words.options("action")?;
                    let files = words.options("files")?;
                    (
                        "api_keys_create",
                        json!({"name": words.required("the key's name")?, "actions": actions, "files": files}),
                    )
                }
                Some("revoke") => (
                    "api_keys_revoke",
                    json!({"keyId": words.required("the key's ID")?}),
                ),
                _ => bail!("{}", usage()),
            };
            http::api_keys_call(state, &deployment, tool, &arguments).await
        }
        "sql" => {
            let sql = words.rest().join(" ");
            ensure!(!sql.trim().is_empty(), "Give the statement in quotes.");
            let body = json!({"operation": "sql", "sql": sql});
            runtime::response_value(admin_database(state, id, body.to_string().as_bytes()).await)
                .await
        }
        "events" => {
            let (from, to) = (words.required_option("from")?, words.required_option("to")?);
            let range = json!({"from": from, "to": to});
            Ok(json!({
                "summary": crate::usage::admin_call(&state.data_root, &deployment, "usage_summary", &range).await?,
                "calls": crate::calls::admin_call(state, &deployment, &json!({})).await?["calls"],
                "notices": crate::mail::notices_tool(state, &deployment, &json!({})).await?["notices"],
                "signals": crate::usage::admin_call(&state.data_root, &deployment, "signals_read", &json!({})).await?["signals"],
                "functionTraces": if state.execution_mode.is_test() { crate::usage::read_function_traces(&state.data_root, &deployment.id, &from, &to)? } else { Vec::new() },
            }))
        }
        _ => bail!("{}", usage()),
    }
}

/// `module_set` on `auth` as Konae's tool runs it. The entries stay when none are given.
async fn signup_policy(
    state: &AppState,
    deployment: Deployment,
    words: &mut Arguments,
) -> Result<Value> {
    let mode: SignupMode = serde_json::from_value(json!(
        words.required("open, invitation, closed or allowlist")?
    ))
    .map_err(|_| anyhow!("The Signup Policy is open, invitation, closed or allowlist."))?;
    let given = words.rest();
    crate::set_signup_policy(
        state,
        &deployment.id,
        mode,
        (!given.is_empty()).then_some(given),
    )
    .await
}

async fn import(state: &AppState, id: &str, words: &mut Arguments) -> Result<Value> {
    use crate::import::{Outcome, Target, import_folder};
    let confirmation = words.option("confirm-schema")?;
    let folder =
        std::path::PathBuf::from(words.required("the exported folder whose data/ to import")?);
    ensure!(words.rest().is_empty(), "{}", usage());
    let served = find_project_deployment(&*state.deployments.read().await, id);
    let deployment = match served {
        Some(deployment) => deployment,
        None => crate::folder::open(state, id, confirmation.as_deref()).await?,
    };
    let _guard = crate::database_lock(state, &format!("project:{}", deployment.project_id)).await;
    let target = Target::sources(state, id)?;
    match import_folder(
        state,
        &deployment,
        &folder,
        &target,
        confirmation.as_deref(),
    )
    .await?
    {
        Outcome::NeedsConfirmation(change) => {
            let mut details = change
                .losses
                .iter()
                .map(|loss| loss.sentence.clone())
                .collect::<Vec<_>>();
            for transition in change.transformations.iter().flatten() {
                details.push(format!(
                    "Transition {} changes {} rows, with a limit of {}.",
                    transition.id, transition.affected_rows, transition.max_rows
                ));
            }
            bail!(
                "The imported data requires schema change confirmation. {} To import it, run again with --confirm-schema {}.",
                details.join(" "),
                change.confirmation().unwrap_or_default()
            )
        }
        Outcome::Imported { backup, change } => Ok(
            json!({"imported": true, "replace": "data", "backup": backup, "schema": change.to_json()}),
        ),
    }
}

fn account_with_email(state: &AppState, id: &str, email: &str) -> Result<String> {
    let path = layout::data(&state.data_root, id).join("identity.sqlite");
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("{} cannot be read.", path.display()))?;
    use rusqlite::OptionalExtension;
    let normalized = crate::auth::normalize_email(email)
        .map_err(|error_value| anyhow!(error_value.to_string()))?;
    db.query_row(
        "SELECT id FROM app_users WHERE email = ?1",
        [normalized],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| anyhow!("This app has no account with the email {}.", email.trim()))
}

fn operator(state: &AppState, id: &str) -> invitations::Inviter {
    let email = Settings::read(&state.data_root, id)
        .ok()
        .and_then(|settings| settings.contact_email);
    invitations::Inviter {
        id: "operator".into(),
        email,
    }
}

async fn served(state: &AppState, id: &str) -> Result<Deployment> {
    find_project_deployment(&*state.deployments.read().await, id)
        .ok_or_else(|| anyhow!("apps/{id} has not been served yet. Start the Kernel on it once, then run the command again."))
}

async fn app_id(state: &AppState, app: Option<String>) -> Result<String> {
    if let Some(id) = app {
        ensure!(
            layout::settings(&state.data_root, &id).exists(),
            "{} holds no app {id}.",
            layout::apps(&state.data_root).display()
        );
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
        [] => bail!(
            "{} holds no app folder.",
            layout::apps(&state.data_root).display()
        ),
        _ => {
            ids.sort();
            bail!("Name the app with --app <id>: {}.", ids.join(", "))
        }
    }
}

struct Arguments {
    words: Vec<String>,
}

impl Arguments {
    fn new(words: &[String]) -> Self {
        Self {
            words: words.to_vec(),
        }
    }

    fn options(&mut self, name: &str) -> Result<Vec<String>> {
        let flag = format!("--{name}");
        let mut values = Vec::new();
        while let Some(index) = self.words.iter().position(|word| *word == flag) {
            ensure!(
                index + 1 < self.words.len() && !self.words[index + 1].starts_with("--"),
                "{flag} needs a value."
            );
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
        self.option(name)?
            .ok_or_else(|| anyhow!("Give --{name}.\n{}", usage()))
    }

    fn next(&mut self) -> Option<String> {
        (!self.words.is_empty()).then(|| self.words.remove(0))
    }

    fn required(&mut self, what: &str) -> Result<String> {
        self.next()
            .ok_or_else(|| anyhow!("Name {what}.\n{}", usage()))
    }

    fn rest(&mut self) -> Vec<String> {
        std::mem::take(&mut self.words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reply_requires_one_bounded_outcome() {
        let (number, outcome) =
            read_test_reply(br#"{"responseNumber":2,"status":200,"body":{"ok":true}}"#.as_slice())
                .unwrap();
        assert_eq!(number, 2);
        assert!(
            matches!(outcome, crate::calls::SyntheticOutcome::Http { status: 200, body } if body == br#"{"ok":true}"#)
        );
        for input in [
            r#"null"#,
            r#"[]"#,
            r#"{"responseNumber":0,"error":"timeout"}"#,
            r#"{"responseNumber":1,"status":600,"body":null}"#,
            r#"{"responseNumber":1,"status":200}"#,
            r#"{"responseNumber":1,"status":200,"body":null,"error":"timeout"}"#,
            r#"{"responseNumber":1,"error":"timeout","extra":true}"#,
            r#"{"responseNumber":1,"error":""}"#,
        ] {
            assert!(read_test_reply(input.as_bytes()).is_err(), "{input}");
        }
        assert!(read_test_reply(vec![b' '; 65_537].as_slice()).is_err());
        assert!(
            matches!(read_test_reply(br#"{"responseNumber":1,"error":"timeout"}"#.as_slice()).unwrap().1, crate::calls::SyntheticOutcome::Network { error } if error == "timeout")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_first_start_writes_a_private_mail_key_and_later_ones_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("konae-mail-key-{}", Uuid::new_v4()));
        let key = mail_key(&root).unwrap();
        assert!(key.len() >= 32 && key != crate::DEFAULT_SETUP_KEY);
        let mode = std::fs::metadata(root.join(MAIL_KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(mail_key(&root).unwrap(), key);
        std::fs::write(root.join(MAIL_KEY_FILE), "short").unwrap();
        assert!(
            mail_key(&root)
                .unwrap_err()
                .to_string()
                .contains("holds no key")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Runs the operator commands both binaries answer: `restore`, the file
/// store commands and `reconcile`. Returns whether one ran.
pub async fn operator_command(installation: &Installation) -> Result<bool> {
    Ok(copies::history::operator_command().await?
        || copies::operator_command(installation).await?
        || file_store::operator_command().await?
        || recovery::operator_command(installation).await?
        || recovery::drill_check(installation).await?)
}
