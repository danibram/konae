//! Eject: the owner takes a Managed Project's sources and the Kernel marks the
//! project External. The Deployment, its accounts, sessions, invitations, mail
//! and URL do not change. ADR-0005 holds the decision and
//! docs/plans/platform-launch.md the behaviour.
//!
//! `eject_plan` pins the saved-sources revision the caller expects and hands
//! back the code package as a download. `eject_apply` flips the project under
//! the same `project:<id>` lock `project_sources_save` takes, so a save from an
//! old tab either lands before the flip, and makes the plan stale, or is
//! refused after it.

use super::mcp::{Owner, digest, download, owned_project, store};
use super::*;
use rusqlite::params;

/// The one answer to a Managed source change after Eject.
pub(super) const CLOSED: &str = "This project was ejected: its sources are maintained outside Littlebox, so the Kernel refuses Managed source changes. Publish a Build Artifact with release_plan and release_apply, or create a new project to edit in Littlebox.";

/** The answer to a project created External that asks to become Managed. */
pub(super) const ALWAYS_EXTERNAL: &str = "This project is External. Littlebox does not take source authority back, so create a new project to edit in Littlebox.";

const PLAN_TTL_MS: u64 = 60 * 60 * 1000;
const PACKAGE_FORMAT: &str = "littlebox-eject";

pub(super) fn install(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS eject_plans(id TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL, sources_revision INTEGER NOT NULL, package_sha256 TEXT NOT NULL, expires INTEGER NOT NULL, receipt TEXT);",
    )?;
    Ok(())
}

pub(super) fn is_ejected(project: &Value) -> bool {
    project["ejected"].is_object()
}

/// The saved-sources revision: how many times the Managed sources were saved.
/// It is not the project's release revision.
pub(super) fn sources_revision(db: &Connection, project: &str) -> Result<u64> {
    Ok(db
        .query_row("SELECT revision FROM managed_sources WHERE project=?1", [project], |r| r.get(0))
        .optional()?
        .unwrap_or(0))
}

/// The project an eject plan names, for the audit row of `eject_apply`.
pub(super) fn plan_project(db: &Connection, plan: &str) -> Option<String> {
    db.query_row("SELECT project FROM eject_plans WHERE id=?1", [plan], |r| r.get(0)).ok()
}

pub(super) fn tools() -> Vec<Value> {
    let text = json!({"type":"string","minLength":1});
    vec![
        super::mcp::tool(
            "eject_plan",
            "Plan an Eject: check that the saved Managed sources are still at expectedRevision and return the code package as a download with its SHA-256. Nothing changes until eject_apply. Requires manage_lifecycle.",
            json!({"projectId":text,"expectedRevision":{"type":"integer","minimum":0}}),
            &["projectId", "expectedRevision"],
            false,
        ),
        super::mcp::tool(
            "eject_apply",
            "Apply an Eject plan: the project becomes External and the Kernel refuses Managed source changes from then on. The Deployment, accounts, sessions, invitations, email and URL stay. Repeating an applied plan returns its receipt. Requires manage_lifecycle.",
            json!({"planId":text}),
            &["planId"],
            false,
        ),
    ]
}

pub(super) async fn admin_call(state: &AppState, owner: &Owner, principal: &str, name: &str, a: &Value) -> Result<Value> {
    match name {
        "eject_plan" => plan(state, owner, principal, a).await,
        "eject_apply" => apply(state, owner, a).await,
        _ => anyhow::bail!("Unknown tool."),
    }
}

/// What Eject changes and what it keeps, in the words of the launch plan.
fn effects() -> Value {
    json!({
        "code": "The project goes from Managed to External.",
        "hostedApp": "Keeps URL, users, sessions, data, invitations and email service.",
        "result": "A conventional project that publishes artifacts through MCP.",
        "returning": "Returning to the managed editor creates a new project, with no automatic merge of production data."
    })
}

async fn plan(state: &AppState, owner: &Owner, principal: &str, a: &Value) -> Result<Value> {
    let id = a["projectId"].as_str().unwrap();
    let expected = a["expectedRevision"].as_u64().unwrap();
    let _guard = database_lock(state, &format!("project:{id}")).await;
    let (project, _) = owned_project(state, owner, id, "manage_lifecycle")?;
    let db = store(state)?;
    let current = sources_revision(&db, id)?;
    if is_ejected(&project) {
        let at = project["ejected"]["sourcesRevision"].as_u64().unwrap_or(0);
        anyhow::ensure!(expected == at, "This project was ejected at sources revision {at}. Plan again with that revision to download its package.");
        // An applied plan stays downloadable, so an interrupted client can
        // fetch the package again and read the receipt.
        let applied: Option<(String, String)> = db
            .query_row(
                "SELECT id,receipt FROM eject_plans WHERE project=?1 AND owner=?2 AND receipt IS NOT NULL",
                params![id, owner.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (plan_id, receipt) = applied.ok_or_else(|| anyhow::anyhow!(CLOSED))?;
        let bytes = package(state, &db, &project, id, at)?;
        let mut answer = plan_value(&plan_id, id, at, &bytes, None)?;
        answer["applied"] = json!(true);
        answer["receipt"] = serde_json::from_str(&receipt)?;
        answer["transfer"] = download(state, principal, &bytes, Some(id)).await?;
        return Ok(answer);
    }
    anyhow::ensure!(
        project["sourceAuthority"] == "managed",
        "Project sources are maintained externally. A project created outside the editor is already External and does not pass through Eject."
    );
    anyhow::ensure!(
        current == expected,
        "Source revision changed: the saved sources are at revision {current}, not {expected}. Reload the project, then plan the Eject again."
    );
    anyhow::ensure!(current > 0, "This project has no saved sources to eject. Save it in the editor, or with project_sources_save, first.");
    let bytes = package(state, &db, &project, id, current)?;
    let sha = digest(&bytes);
    let now = now_ms();
    // A current plan for the same revision is reused, so a retried plan hands
    // out the same plan ID with a fresh download.
    let reused: Option<(String, u64)> = db
        .query_row(
            "SELECT id,expires FROM eject_plans WHERE project=?1 AND owner=?2 AND sources_revision=?3 AND package_sha256=?4 AND receipt IS NULL AND expires>?5",
            params![id, owner.id, current, sha, now],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (plan_id, expires) = match reused {
        Some(row) => row,
        None => {
            let plan_id = Uuid::new_v4().to_string();
            let expires = now + PLAN_TTL_MS;
            db.execute(
                "INSERT INTO eject_plans(id,owner,project,sources_revision,package_sha256,expires) VALUES(?1,?2,?3,?4,?5,?6)",
                params![plan_id, owner.id, id, current, sha, expires],
            )?;
            (plan_id, expires)
        }
    };
    let mut answer = plan_value(&plan_id, id, current, &bytes, Some(expires))?;
    answer["applied"] = json!(false);
    answer["transfer"] = download(state, principal, &bytes, Some(id)).await?;
    Ok(answer)
}

fn plan_value(plan_id: &str, id: &str, revision: u64, bytes: &[u8], expires: Option<u64>) -> Result<Value> {
    let body: Value = serde_json::from_slice(bytes)?;
    Ok(json!({
        "planId": plan_id,
        "projectId": id,
        "expectedRevision": revision,
        "expiresAt": expires,
        "package": {"format": PACKAGE_FORMAT, "size": bytes.len(), "sha256": digest(bytes), "contents": body["contents"]},
        "effects": effects(),
    }))
}

async fn apply(state: &AppState, owner: &Owner, a: &Value) -> Result<Value> {
    let plan_id = a["planId"].as_str().unwrap();
    let project_id: String = store(state)?
        .query_row(
            "SELECT project FROM eject_plans WHERE id=?1 AND owner=?2",
            params![plan_id, owner.id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("Eject plan not found."))?;
    let _guard = database_lock(state, &format!("project:{project_id}")).await;
    let (mut project, _) = owned_project(state, owner, &project_id, "manage_lifecycle")?;
    let mut db = store(state)?;
    let (revision, sha, expires, receipt): (u64, String, u64, Option<String>) = db.query_row(
        "SELECT sources_revision,package_sha256,expires,receipt FROM eject_plans WHERE id=?1",
        [plan_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if let Some(receipt) = receipt {
        return Ok(serde_json::from_str(&receipt)?);
    }
    anyhow::ensure!(!is_ejected(&project), "This project was already ejected with another plan.");
    anyhow::ensure!(project["sourceAuthority"] == "managed", "Project sources are maintained externally.");
    anyhow::ensure!(expires > now_ms(), "Eject plan expired. Plan the Eject again.");
    let current = sources_revision(&db, &project_id)?;
    anyhow::ensure!(
        current == revision,
        "Source revision changed since this plan: the saved sources are at revision {current}, not {revision}. Plan the Eject again."
    );
    let deployment = find_project_deployment(&*state.deployments.read().await, &project_id);
    let at = now_ms();
    let via = if owner.session { "session" } else { "token" };
    project["sourceAuthority"] = json!("external");
    project["ejected"] = json!({"at": at, "by": owner.id, "via": via, "planId": plan_id, "sourcesRevision": revision});
    let receipt = json!({
        "planId": plan_id,
        "projectId": project_id,
        "sourceAuthority": "external",
        "sourcesRevision": revision,
        "packageSha256": sha,
        "ejectedAt": at,
        "ejectedBy": owner.id,
        "via": via,
        "deployment": deployment.as_ref().map(|d| json!({"id": d.id, "url": d.url})),
        "effects": effects(),
        "publish": publish_commands(),
    });
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE projects SET body=?1 WHERE id=?2 AND owner=?3",
        params![project.to_string(), project_id, owner.id],
    )?;
    tx.execute("UPDATE eject_plans SET receipt=?1 WHERE id=?2", params![receipt.to_string(), plan_id])?;
    tx.commit()?;
    Ok(receipt)
}

fn publish_commands() -> Value {
    json!([
        "bun run littlebox -- login --dir <dir> --url <url>",
        "bun run littlebox -- build --dir <dir>",
        "bun run littlebox -- dev --dir <dir>",
        "bun run littlebox -- publish --dir <dir>"
    ])
}

/// The code package: the saved sources at `revision` as files, plus the
/// `littlebox.json` of an External project and a README. It is built from the
/// stored bytes on every call and holds no clock, so each download of one
/// revision has the same SHA-256.
fn package(state: &AppState, db: &Connection, project: &Value, id: &str, revision: u64) -> Result<Vec<u8>> {
    let body: Vec<u8> = db
        .query_row(
            "SELECT body FROM managed_sources WHERE project=?1 AND revision=?2",
            params![id, revision],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("The saved sources changed. Plan the Eject again."))?;
    let source: Value = serde_json::from_slice(&body)?;
    let mut files: std::collections::BTreeMap<String, String> = source["files"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid project files."))?
        .iter()
        .map(|(path, contents)| (path.clone(), contents.as_str().unwrap_or("").to_owned()))
        .collect();
    let model = source["databaseModel"]["source"].as_str().unwrap_or("");
    if !files.contains_key("schema.ts") && !model.is_empty() {
        files.insert("schema.ts".into(), model.to_owned());
    }
    let name = source["name"].as_str().or(project["name"].as_str()).unwrap_or(id).to_owned();
    let slug = source["slug"].as_str().or(project["slug"].as_str()).unwrap_or(id).to_owned();
    let origin = std::env::var("LITTLEBOX_MCP_ORIGIN")
        .map(|origin| origin.trim_end_matches('/').to_owned())
        .unwrap_or_else(|_| state.public_origin.clone());
    let config = json!({"url": origin, "project": {"id": id, "name": name, "slug": slug, "sourceAuthority": "external", "manifest": source["manifest"]}});
    anyhow::ensure!(!files.contains_key("littlebox.json"), "The saved sources hold a littlebox.json, which Eject writes itself. Rename that file in the editor first.");
    files.insert("littlebox.json".into(), format!("{}\n", serde_json::to_string_pretty(&config)?));
    let guide = if files.contains_key("README.md") { "LITTLEBOX.md" } else { "README.md" };
    anyhow::ensure!(!files.contains_key(guide), "The saved sources hold README.md and LITTLEBOX.md. Rename one in the editor first.");
    files.insert(guide.into(), readme(&name, revision, &origin));
    let contents: Vec<Value> = files
        .iter()
        .map(|(path, text)| json!({"path": path, "size": text.len(), "sha256": digest(text.as_bytes())}))
        .collect();
    let directories = source["directories"].clone();
    Ok(serde_json::to_vec(&json!({
        "format": PACKAGE_FORMAT,
        "version": 1,
        "projectId": id,
        "sourcesRevision": revision,
        "contents": contents,
        "directories": if directories.is_array() { directories } else { json!([]) },
        "files": files,
    }))?)
}

fn readme(name: &str, revision: u64, url: &str) -> String {
    format!(
        "# {name}\n\n\
These are the sources of {name} as Littlebox saved them at revision {revision}, when the project was ejected. Littlebox no longer edits them. The editor and its agent show the project read-only, and the Kernel refuses Managed source changes from any tab or agent.\n\n\
The hosted app did not change. It keeps its URL, users, sessions, data, files, invitations and email service. Publishing from this folder replaces only its Build Artifact.\n\n\
## Publish from this folder\n\n\
Run these from a Littlebox checkout. `login` reads `LITTLEBOX_EMAIL` and `LITTLEBOX_PASSWORD` and saves a 24-hour credential in `.littlebox/`, which stays out of Git.\n\n\
```sh\n\
bun run littlebox -- login --dir <this folder> --url {url}\n\
bun run littlebox -- build --dir <this folder>\n\
bun run littlebox -- dev --dir <this folder>\n\
bun run littlebox -- publish --dir <this folder>\n\
```\n\n\
`dev` runs the same Kernel locally with its own accounts and data. `publish` sends a Build Artifact through MCP with `release_plan` and `release_apply`.\n\n\
A publish never changes who may create an app account. That stays in the editor's Auth > Users view and `signup_policy_set`. `project.manifest.signupPolicy` in `littlebox.json` names the policy of a first release only. This app has been released, so a publish from this folder sends no policy and warns when the file names a different one.\n\n\
## Schema\n\n\
`schema.sql` is what the Kernel runs on every publish. It accepts additive changes only: new tables and indexes with `IF NOT EXISTS`. `schema.ts` is the Database Model the editor planned migrations from. Nothing reads it now, so the schema, its migrations and their compatibility with live data are yours. Back up the data before changing an existing table.\n\n\
## Going back\n\n\
Littlebox does not take a project back into the editor. To edit in Littlebox again, create a new project. Production data does not merge into it.\n"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_refusal_names_the_way_out() {
        assert!(super::CLOSED.contains("release_plan"));
        assert!(super::CLOSED.contains("create a new project"));
    }

    #[test]
    fn the_guide_lists_the_publish_commands() {
        let text = super::readme("Notes", 3, "https://example.test");
        assert!(text.contains("revision 3"));
        assert!(text.contains("littlebox -- publish"));
        assert!(text.contains("A publish never changes who may create an app account"), "the guide says where the signup policy changes");
        assert!(text.contains("--url https://example.test"));
    }
}
