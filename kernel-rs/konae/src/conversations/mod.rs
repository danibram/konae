//! Dormant, owner-only transport for browser-owned Pi Durable 1.5.2 storage.
//!
//! This module stores the public StorageWrite records verbatim as ordered
//! batches. It does not run Pi's harness or introduce a server-side scheduler.
//! These tables live in the installation's mcp.sqlite root database, which is
//! copied with the installation and is deliberately outside app-folder export.
//! RawValue keeps opaque JSON slices intact across write, replay and download,
//! including JSON-escaped lone UTF-16 code units.

use super::*;
use axum::{
    body::to_bytes,
    extract::{Path as AxumPath, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use sha2::{Digest, Sha256};

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const DETAIL_LIMIT: usize = 1024 * 1024;
const OWNER_METADATA_LIMIT: i64 = 16 * 1024 * 1024;
const OWNER_BLOB_LIMIT: i64 = 64 * 1024 * 1024;
const BLOB_LIMIT: usize = 20 * 1024 * 1024;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const SUPPORTED_FORMAT: i64 = 1;
const SUPPORTED_PI_VERSION: &str = "1.5.2";
// Registry "1" is the only tool/document registry this prototype may resume.
// Other values remain opaque and readable through the owner timeline.
const SUPPORTED_REGISTRY_VERSION: &str = "1";

pub(super) fn routes() -> Router<Konae> {
    Router::new()
        .route(
            "/_konae/projects/{project}/conversations",
            get(list).post(create),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}",
            get(read).delete(remove),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/claim",
            post(claim),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/renew",
            post(renew),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/lease",
            get(lease_status),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/release",
            post(release),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/stop",
            post(request_stop),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/effects",
            get(list_effects),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/effects/{call_id}",
            get(read_effect).post(settle_effect),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/commit",
            post(commit),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/ids",
            post(mint_id),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/records",
            get(records),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/records/{record}/details",
            get(details),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/batches",
            get(batches),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/receipts/{request_id}",
            get(receipt),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/checkpoints",
            get(checkpoints).post(checkpoint),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/blobs/{digest}",
            get(blob).put(put_blob),
        )
        .route(
            "/_konae/projects/{project}/conversations/{conversation}/download",
            get(download),
        )
}

#[cfg(test)]
mod agent_sql_classification_tests {
    use super::agent_sql_is_read_only;

    #[test]
    fn accepts_a_single_select_with_a_trailing_semicolon() {
        assert!(agent_sql_is_read_only(" SELECT id FROM notes LIMIT 10;  "));
    }

    #[test]
    fn refuses_multiple_statements_and_non_select_sql() {
        assert!(!agent_sql_is_read_only("SELECT id FROM notes LIMIT 10; DELETE FROM notes"));
        assert!(!agent_sql_is_read_only("WITH rows AS (SELECT 1) SELECT * FROM rows LIMIT 1"));
        assert!(!agent_sql_is_read_only("DELETE FROM notes"));
    }

    #[test]
    fn effect_read_back_requires_a_numeric_bounded_limit() {
        assert!(super::agent_sql_is_bounded_read("SELECT id FROM notes LIMIT 50;"));
        assert!(super::agent_sql_is_bounded_read("SELECT id FROM notes LIMIT 10 OFFSET 40"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes LIMIT 1001"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes -- LIMIT 10"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes /* LIMIT 10 */"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes WHERE title = 'a LIMIT 10 string'"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM (SELECT id FROM notes LIMIT 10)"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes LIMIT 1+1000"));
        assert!(!super::agent_sql_is_bounded_read("SELECT id FROM notes LIMIT 1, 1001"));
    }
}

pub(super) fn schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS conversation_project_epochs(
          owner TEXT NOT NULL, project TEXT NOT NULL, epoch INTEGER NOT NULL,
          PRIMARY KEY(owner,project));
        CREATE TABLE IF NOT EXISTS conversation_id_highwater(
          owner TEXT NOT NULL, project TEXT NOT NULL, high_water INTEGER NOT NULL,
          PRIMARY KEY(owner,project));
        CREATE TABLE IF NOT EXISTS conversation_used_ids(
          owner TEXT NOT NULL, project TEXT NOT NULL, id INTEGER NOT NULL,
          conversation TEXT NOT NULL, PRIMARY KEY(owner,project,id));
        CREATE TABLE IF NOT EXISTS conversations(
          id TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL,
          project_epoch INTEGER NOT NULL DEFAULT 1,
          format_version INTEGER NOT NULL, pi_version TEXT NOT NULL,
          registry_version TEXT NOT NULL, title TEXT NOT NULL,
          created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
          byte_size INTEGER NOT NULL DEFAULT 0,
          current_generation INTEGER NOT NULL DEFAULT 0, last_seq INTEGER NOT NULL DEFAULT 0,
          deleted INTEGER NOT NULL DEFAULT 0, UNIQUE(owner,project,id));
        CREATE INDEX IF NOT EXISTS conversations_owner_project ON conversations(owner,project,updated_at DESC);
        CREATE TABLE IF NOT EXISTS conversation_live_authority(
          conversation TEXT PRIMARY KEY, owner TEXT NOT NULL, project TEXT NOT NULL,
          generation INTEGER NOT NULL, claimed_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS conversation_execution_leases(
          owner TEXT NOT NULL, project TEXT NOT NULL, conversation TEXT NOT NULL,
          executor_id TEXT NOT NULL, generation INTEGER NOT NULL,
          expires_at INTEGER NOT NULL, PRIMARY KEY(owner,project));
        CREATE TABLE IF NOT EXISTS conversation_project_generations(
          owner TEXT NOT NULL, project TEXT NOT NULL, generation INTEGER NOT NULL,
          PRIMARY KEY(owner,project));
        CREATE TABLE IF NOT EXISTS conversation_stop_requests(
          owner TEXT NOT NULL, project TEXT NOT NULL, conversation TEXT NOT NULL,
          executor_id TEXT NOT NULL, generation INTEGER NOT NULL,
          request_id TEXT NOT NULL, created_at INTEGER NOT NULL,
          PRIMARY KEY(owner,project));
        CREATE TABLE IF NOT EXISTS conversation_effects(
          owner TEXT NOT NULL, project TEXT NOT NULL, conversation TEXT NOT NULL,
          call_id TEXT NOT NULL, executor_id TEXT NOT NULL, generation INTEGER NOT NULL,
          tool TEXT NOT NULL, effect_class TEXT NOT NULL, args_hash TEXT NOT NULL,
          intent TEXT, state TEXT NOT NULL, result TEXT, refusal TEXT,
          observation_call_id TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
          PRIMARY KEY(owner,project,call_id));
        CREATE TABLE IF NOT EXISTS conversation_batches(
          conversation TEXT NOT NULL, owner TEXT NOT NULL, project TEXT NOT NULL,
          request_id TEXT NOT NULL, content_hash TEXT NOT NULL,
          expected_seq INTEGER NOT NULL, generation INTEGER NOT NULL, receipt TEXT NOT NULL,
          created_at INTEGER NOT NULL, PRIMARY KEY(conversation,request_id));
        CREATE TABLE IF NOT EXISTS conversation_records(
          conversation TEXT NOT NULL, seq INTEGER NOT NULL, ordinal INTEGER NOT NULL,
          record_id INTEGER, record_type TEXT NOT NULL, format_version INTEGER NOT NULL,
          payload TEXT NOT NULL, byte_size INTEGER NOT NULL,
          PRIMARY KEY(conversation,seq,ordinal));
        CREATE UNIQUE INDEX IF NOT EXISTS conversation_record_ids
          ON conversation_records(conversation,record_id) WHERE record_id IS NOT NULL;
        CREATE TABLE IF NOT EXISTS conversation_checkpoints(
          conversation TEXT NOT NULL, checkpoint_id TEXT NOT NULL, seq INTEGER NOT NULL,
          payload TEXT NOT NULL, byte_size INTEGER NOT NULL, created_at INTEGER NOT NULL,
          PRIMARY KEY(conversation,checkpoint_id));
        CREATE TABLE IF NOT EXISTS conversation_blob_refs(
          conversation TEXT NOT NULL, digest TEXT NOT NULL, size INTEGER NOT NULL,
          PRIMARY KEY(conversation,digest));
        CREATE TABLE IF NOT EXISTS conversation_blobs(
          digest TEXT PRIMARY KEY, bytes BLOB NOT NULL, size INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS conversation_tombstones(
          owner TEXT NOT NULL, project TEXT NOT NULL, conversation TEXT NOT NULL,
          deleted_at INTEGER NOT NULL, PRIMARY KEY(owner,project,conversation));"
    )
    ?;
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(conversations)")?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|c| c == "project_epoch") {
        db.execute(
            "ALTER TABLE conversations ADD COLUMN project_epoch INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }
    if !columns.iter().any(|c| c == "byte_size") {
        db.execute(
            "ALTER TABLE conversations ADD COLUMN byte_size INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn metadata_bytes(db: &Connection, owner: &str) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT COALESCE((SELECT SUM(c.byte_size) FROM conversations c WHERE c.owner=?1),0)+COALESCE((SELECT SUM(r.byte_size) FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE c.owner=?1),0)+COALESCE((SELECT SUM(p.byte_size) FROM conversation_checkpoints p JOIN conversations c ON c.id=p.conversation WHERE c.owner=?1),0)+COALESCE((SELECT SUM(length(b.receipt)) FROM conversation_batches b WHERE b.owner=?1),0)+COALESCE((SELECT SUM(length(e.call_id)+length(e.executor_id)+length(e.project)+length(e.conversation)+length(e.tool)+length(e.effect_class)+length(e.args_hash)+length(e.state)+length(COALESCE(e.refusal,''))+512) FROM conversation_effects e WHERE e.owner=?1),0)",
        [owner],
        |r| r.get(0),
    )
}

async fn owner_request(
    state: &Konae,
    headers: HeaderMap,
    project: &str,
) -> Result<(String, Connection), Response> {
    if !kernel::same_origin(&headers)
        || !kernel::runtime::valid_host(&state.kernel, &headers)
        || super::security::from_app_origin(state, &headers).await
        || !super::owners::oauth::valid_owner_host(&state, &headers).await
    {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Cross-origin or app-origin conversation request rejected.",
        ));
    }
    if headers.get(header::AUTHORIZATION).is_some() {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Conversation history requires an owner browser session.",
        ));
    }
    let owner = match super::mcp::authenticate_owner(state, &headers) {
        Ok(Some(owner)) if owner.session => owner,
        Ok(_) => {
            return Err(fail(
                StatusCode::UNAUTHORIZED,
                "An owner browser session is required.",
            ))
        }
        Err(_) => {
            return Err(fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "Owner identity is unavailable.",
            ))
        }
    };
    if headers
        .get("x-konae-owner-id")
        .is_some_and(|expected| expected.as_bytes() != owner.id.as_bytes())
    {
        return Err(fail(
            StatusCode::CONFLICT,
            "The signed-in owner changed. Reload before continuing.",
        ));
    }
    let db = super::mcp::store(state).map_err(internal)?;
    schema(&db).map_err(internal)?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1 AND owner=?2)",
            params![project, owner.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    if !exists {
        return Err(fail(
            StatusCode::NOT_FOUND,
            "Project is not available to this owner.",
        ));
    }
    Ok((owner.id, db))
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn internal(error: impl std::fmt::Display) -> Response {
    fail(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
}
fn json_response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
fn raw_response(body: String) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    response
}

#[derive(Deserialize)]
struct Page {
    before: Option<i64>,
    before_id: Option<String>,
    limit: Option<usize>,
}

async fn list(
    State(state): State<Konae>,
    AxumPath(project): AxumPath<String>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(value) => value,
        Err(e) => return e,
    };
    let page = Query::<Page>::try_from_uri(request.uri())
        .map(|q| q.0)
        .unwrap_or(Page {
            before: None,
            before_id: None,
            limit: None,
        });
    let limit = page.limit.unwrap_or(50);
    if !(1..=50).contains(&limit) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Conversation page limit must be between 1 and 50.",
        );
    }
    let before = page.before.unwrap_or(MAX_SAFE_INTEGER);
    match db.prepare("SELECT id,format_version,pi_version,registry_version,title,created_at,updated_at,last_seq,current_generation FROM conversations WHERE owner=?1 AND project=?2 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?1 AND project=?2) AND deleted=0 AND (updated_at<?3 OR (updated_at=?3 AND id<?4)) ORDER BY updated_at DESC,id DESC LIMIT ?5") {
        Ok(mut q) => match q.query_map(params![owner,project,before,page.before_id.unwrap_or_default(),limit as i64], |r| Ok(json!({"id":r.get::<_,String>(0)?,"formatVersion":r.get::<_,i64>(1)?,"piVersion":r.get::<_,String>(2)?,"registryVersion":r.get::<_,String>(3)?,"title":r.get::<_,String>(4)?,"createdAt":r.get::<_,i64>(5)?,"updatedAt":r.get::<_,i64>(6)?,"lastSeq":r.get::<_,i64>(7)?,"generation":r.get::<_,i64>(8)?}))) {
            Ok(rows) => match rows.collect::<rusqlite::Result<Vec<_>>>() { Ok(items) => {
                let next=if items.len()==limit {items.last().map(|v|json!({"before":v["updatedAt"],"beforeId":v["id"]}))}else{None};
                json_response(json!({"items":items,"nextCursor":next}))
            }, Err(e) => internal(e) },
            Err(e) => internal(e),
        }, Err(e) => internal(e)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateConversation {
    id: String,
    format_version: i64,
    pi_version: String,
    registry_version: String,
    title: Option<String>,
}

async fn create(
    State(state): State<Konae>,
    AxumPath(project): AxumPath<String>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), 16 * 1024).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Conversation envelope exceeds 16 KiB.",
            )
        }
    };
    let input: CreateConversation = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid conversation envelope."),
    };
    if input.id.is_empty()
        || input.id.len() > 128
        || input.format_version <= 0
        || input.pi_version.len() > 64
        || input.registry_version.len() > 128
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Conversation ID or version fields are invalid.",
        );
    }
    let now = now_ms();
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => return internal(e),
    };
    let _ = tx.execute(
        "INSERT OR IGNORE INTO conversation_project_epochs(owner,project,epoch) VALUES(?1,?2,1)",
        params![owner, project],
    );
    let existing_metadata = match metadata_bytes(&tx, &owner) {
        Ok(value) => value,
        Err(error) => return internal(error),
    };
    if existing_metadata.saturating_add(bytes.len() as i64) > OWNER_METADATA_LIMIT {
        return fail(StatusCode::INSUFFICIENT_STORAGE, "Owner conversation metadata quota is 16 MiB. Delete or download conversations before creating another conversation.");
    }
    let inserted=tx.execute("INSERT OR IGNORE INTO conversations(id,owner,project,project_epoch,format_version,pi_version,registry_version,title,created_at,updated_at,byte_size) SELECT ?1,?2,?3,(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3),?4,?5,?6,?7,?8,?8,?9 WHERE EXISTS(SELECT 1 FROM projects WHERE id=?3 AND owner=?2) AND NOT EXISTS(SELECT 1 FROM conversation_tombstones WHERE owner=?2 AND project=?3 AND conversation=?1)",params![input.id,owner,project,input.format_version,input.pi_version,input.registry_version,input.title.unwrap_or_default(),now,bytes.len() as i64]);
    match inserted {
        Ok(1) => {
            if let Err(e) = tx.commit() {
                return internal(e);
            };
            json_response(json!({"id":input.id,"createdAt":now,"lastSeq":0,"generation":0}))
        }
        Ok(_) => fail(
            StatusCode::CONFLICT,
            "Conversation ID already exists or is tombstoned.",
        ),
        Err(e) => internal(e),
    }
}

async fn read(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    match db.query_row("SELECT id,format_version,pi_version,registry_version,title,created_at,updated_at,last_seq,current_generation FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND deleted=0",params![conversation,owner,project],|r|Ok(json!({"id":r.get::<_,String>(0)? ,"formatVersion":r.get::<_,i64>(1)? ,"piVersion":r.get::<_,String>(2)? ,"registryVersion":r.get::<_,String>(3)? ,"title":r.get::<_,String>(4)? ,"createdAt":r.get::<_,i64>(5)? ,"updatedAt":r.get::<_,i64>(6)? ,"lastSeq":r.get::<_,i64>(7)? ,"generation":r.get::<_,i64>(8)?}))) { Ok(v)=>json_response(v),Err(_)=>fail(StatusCode::NOT_FOUND,"Conversation is not available.") }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Claim {
    expected_generation: i64,
    executor_id: String,
    continue_explicitly: bool,
}
async fn claim(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Claim request exceeds 4 KiB.",
            )
        }
    };
    let input: Claim = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid generation claim."),
    };
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let current:Option<(i64,i64,String,String)>=tx.query_row("SELECT current_generation,format_version,pi_version,registry_version FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND deleted=0",params![conversation,owner,project],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().unwrap_or(None);
    let Some((_current, format, pi, registry)) = current else {
        return fail(StatusCode::NOT_FOUND, "Conversation is not available.");
    };
    if format != SUPPORTED_FORMAT
        || pi != SUPPORTED_PI_VERSION
        || registry != SUPPORTED_REGISTRY_VERSION
    {
        return fail(StatusCode::CONFLICT,"This conversation record or registry version is readable but cannot be resumed by this Konae storage adapter.");
    }
    if input.executor_id.is_empty() || input.executor_id.len() > 128 || !input.continue_explicitly {
        return fail(
            StatusCode::BAD_REQUEST,
            "A stable executor ID and explicit Continue are required to claim execution.",
        );
    }
    let now = now_ms();
    let lease: Option<(i64, i64)> = tx.query_row(
        "SELECT generation,expires_at FROM conversation_execution_leases WHERE owner=?1 AND project=?2",
        params![owner, project], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional().unwrap_or(None);
    if lease.is_some_and(|(_, expires)| expires > now) {
        return fail(
            StatusCode::CONFLICT,
            "Another executor still holds this project's live lease.",
        );
    }
    let project_generation: i64 = tx
        .query_row(
            "SELECT generation FROM conversation_project_generations WHERE owner=?1 AND project=?2",
            params![owner, project],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None)
        .unwrap_or(0);
    if project_generation != input.expected_generation {
        return fail(StatusCode::CONFLICT, "Project execution generation changed; reload the latest receipts and checkpoint before continuing.");
    }
    let unresolved: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_effects WHERE owner=?1 AND project=?2 AND state IN ('admitted','running','unknown'))",
        params![owner, project], |r| r.get(0),
    ).unwrap_or(true);
    if unresolved {
        return fail(
            StatusCode::CONFLICT,
            "An admitted or ambiguous write must be reconciled before this project can continue.",
        );
    }
    let Some(next) = project_generation
        .checked_add(1)
        .filter(|v| *v <= MAX_SAFE_INTEGER)
    else {
        return fail(
            StatusCode::CONFLICT,
            "Conversation generation namespace exhausted.",
        );
    };
    let expires = now.saturating_add(60_000);
    if let Err(e)=tx.execute("INSERT INTO conversation_project_generations(owner,project,generation) VALUES(?1,?2,?3) ON CONFLICT(owner,project) DO UPDATE SET generation=excluded.generation",params![owner,project,next]) {return internal(e)}
    if let Err(e) = tx.execute("INSERT INTO conversation_execution_leases(owner,project,conversation,executor_id,generation,expires_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(owner,project) DO UPDATE SET conversation=excluded.conversation,executor_id=excluded.executor_id,generation=excluded.generation,expires_at=excluded.expires_at", params![owner, project, conversation, input.executor_id, next, expires]) { return internal(e); }
    let _ = tx.execute(
        "DELETE FROM conversation_stop_requests WHERE owner=?1 AND project=?2",
        params![owner, project],
    );
    if let Err(e) = tx.execute(
        "UPDATE conversations SET current_generation=?2,updated_at=?3 WHERE id=?1",
        params![conversation, next, now],
    ) {
        return internal(e);
    }
    if let Err(e)=tx.execute("INSERT INTO conversation_live_authority(conversation,owner,project,generation,claimed_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(conversation) DO UPDATE SET owner=excluded.owner,project=excluded.project,generation=excluded.generation,claimed_at=excluded.claimed_at",params![conversation,owner,project,next,now]){return internal(e)}
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(
        json!({"generation":next,"claimedAt":now,"expiresAt":expires,"renewAfterMs":15_000,"executorId":input.executor_id}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LeaseIdentity {
    executor_id: String,
    generation: i64,
}

async fn renew(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    lease_change(state, project, conversation, request, false).await
}
async fn lease_status(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(r) = require_conversation(&db, &owner, &project, &conversation) {
        return r;
    }
    let now = now_ms();
    let lease:Option<(String,String,i64,i64)>=match db.query_row("SELECT conversation,executor_id,generation,expires_at FROM conversation_execution_leases WHERE owner=?1 AND project=?2",params![owner,project],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional(){Ok(v)=>v,Err(e)=>return internal(e)};
    let unresolved:bool=match db.query_row("SELECT EXISTS(SELECT 1 FROM conversation_effects WHERE owner=?1 AND project=?2 AND state IN ('admitted','running','unknown'))",params![owner,project],|r|r.get(0)){Ok(v)=>v,Err(e)=>return internal(e)};
    let generation: i64 = match db
        .query_row(
            "SELECT generation FROM conversation_project_generations WHERE owner=?1 AND project=?2",
            params![owner, project],
            |r| r.get(0),
        )
        .optional()
    {
        Ok(Some(v)) => v,
        Ok(None) => 0,
        Err(e) => return internal(e),
    };
    let live = lease.as_ref().is_some_and(|v| v.3 > now);
    let stop_requested:bool=match db.query_row("SELECT EXISTS(SELECT 1 FROM conversation_stop_requests WHERE owner=?1 AND project=?2 AND conversation=?3)",params![owner,project,conversation],|r|r.get(0)){Ok(v)=>v,Err(e)=>return internal(e)};
    json_response(
        json!({"generation":generation,"conversationId":lease.as_ref().map(|v|v.0.as_str()),"executorId":lease.as_ref().map(|v|v.1.as_str()),"expiresAt":lease.map(|v|v.3),"live":live,"unresolvedEffects":unresolved,"stopRequested":stop_requested,"serverNow":now}),
    )
}
async fn release(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    lease_change(state, project, conversation, request, true).await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopRequest {
    request_id: String,
    generation: i64,
}
async fn request_stop(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(r) = require_conversation(&db, &owner, &project, &conversation) {
        return r;
    }
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "Stop request exceeds 4 KiB."),
    };
    let input: StopRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid project stop request."),
    };
    if input.request_id.is_empty() || input.request_id.len() > 128 {
        return fail(StatusCode::BAD_REQUEST, "Stop request ID is invalid.");
    }
    let now = now_ms();
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let live:Option<(String,i64)>=tx.query_row("SELECT executor_id,generation FROM conversation_execution_leases WHERE owner=?1 AND project=?2 AND conversation=?3 AND generation=?4 AND expires_at>?5",params![owner,project,conversation,input.generation,now],|r|Ok((r.get(0)?,r.get(1)?))).optional().unwrap_or(None);
    let Some((executor, generation)) = live else {
        return fail(
            StatusCode::CONFLICT,
            "There is no live project executor to request stop from.",
        );
    };
    if let Err(e)=tx.execute("INSERT INTO conversation_stop_requests(owner,project,conversation,executor_id,generation,request_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(owner,project) DO UPDATE SET conversation=excluded.conversation,executor_id=excluded.executor_id,generation=excluded.generation,request_id=excluded.request_id,created_at=excluded.created_at",params![owner,project,conversation,executor,generation,input.request_id,now]){return internal(e)}
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(
        json!({"requested":true,"requestId":input.request_id,"executorId":executor,"generation":generation,"createdAt":now}),
    )
}
async fn lease_change(
    state: Konae,
    project: String,
    conversation: String,
    request: Request,
    release: bool,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Lease request exceeds 4 KiB.",
            )
        }
    };
    let input: LeaseIdentity = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid lease identity."),
    };
    let now = now_ms();
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let live: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_execution_leases WHERE owner=?1 AND project=?2 AND conversation=?3 AND executor_id=?4 AND generation=?5 AND expires_at>?6)",params![owner,project,conversation,input.executor_id,input.generation,now],|r|r.get(0)).unwrap_or(false);
    if !live {
        return fail(
            StatusCode::CONFLICT,
            "Project execution lease is expired or fenced.",
        );
    }
    if release {
        let running:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_effects WHERE owner=?1 AND project=?2 AND state='running')",params![owner,project],|r|r.get(0)).unwrap_or(true);
        if running {
            return fail(StatusCode::CONFLICT,"Project handoff is waiting for an admitted MCP write to finish or become observable.");
        }
    }
    let expires = if release {
        now
    } else {
        now.saturating_add(60_000)
    };
    if let Err(e)=tx.execute("UPDATE conversation_execution_leases SET expires_at=?6 WHERE owner=?1 AND project=?2 AND conversation=?3 AND executor_id=?4 AND generation=?5",params![owner,project,conversation,input.executor_id,input.generation,expires]) { return internal(e); }
    if release {
        let _=tx.execute("DELETE FROM conversation_live_authority WHERE conversation=?1 AND owner=?2 AND project=?3 AND generation=?4",params![conversation,owner,project,input.generation]);
        let _=tx.execute("DELETE FROM conversation_stop_requests WHERE owner=?1 AND project=?2 AND generation=?3",params![owner,project,input.generation]);
    }
    let stop_requested:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_stop_requests WHERE owner=?1 AND project=?2 AND conversation=?3 AND executor_id=?4 AND generation=?5)",params![owner,project,conversation,input.executor_id,input.generation],|r|r.get(0)).unwrap_or(false);
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(
        json!({"generation":input.generation,"expiresAt":expires,"released":release,"stopRequested":stop_requested}),
    )
}

async fn list_effects(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let active = match active_agent_effects().lock() {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "Project effect registry is unavailable."),
    };
    match db.prepare("SELECT call_id,executor_id,generation,tool,effect_class,state,observation_call_id,created_at,updated_at,intent,refusal FROM conversation_effects WHERE owner=?1 AND project=?2 AND conversation=?3 ORDER BY created_at DESC,call_id DESC LIMIT 50") {
        Ok(mut q)=>match q.query_map(params![owner,project,conversation],|r| {
            let call_id: String = r.get(0)?;
            let intent: Option<String> = r.get(9)?;
            let stored_state: String = r.get(5)?;
            let visible_state = if stored_state == "running" && !active.contains(&effect_key(&owner, &project, &call_id)) { "unknown" } else { &stored_state };
            Ok(json!({"callId":call_id,"executorId":r.get::<_,String>(1)?,"generation":r.get::<_,i64>(2)? ,"tool":r.get::<_,String>(3)? ,"effectClass":r.get::<_,String>(4)? ,"state":visible_state,"observationCallId":r.get::<_,Option<String>>(6)? ,"createdAt":r.get::<_,i64>(7)? ,"updatedAt":r.get::<_,i64>(8)? ,"intent":intent.and_then(|value|serde_json::from_str::<Value>(&value).ok()),"refusal":r.get::<_,Option<String>>(10)?}))
        }) {
            Ok(rows)=>match rows.collect::<rusqlite::Result<Vec<_>>>() {Ok(items)=>json_response(json!({"items":items,"limit":50})),Err(e)=>internal(e)},Err(e)=>internal(e)
        },Err(e)=>internal(e)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectSettlement {
    state: String,
    result: Option<Box<RawValue>>,
    refusal: Option<String>,
    observation_call_id: Option<String>,
    acknowledgement: Option<String>,
}
async fn read_effect(
    State(state): State<Konae>,
    AxumPath((project, conversation, call_id)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let active = match active_agent_effects().lock() {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "Project effect registry is unavailable."),
    };
    match db.query_row("SELECT tool,effect_class,state,result,refusal,observation_call_id,created_at,updated_at FROM conversation_effects WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4",params![owner,project,conversation,call_id],|r|{
        let stored_state: String = r.get(2)?;
        let visible_state = if stored_state == "running" && !active.contains(&effect_key(&owner, &project, &call_id)) { "unknown" } else { &stored_state };
        Ok(json!({"callId":call_id,"tool":r.get::<_,String>(0)? ,"effectClass":r.get::<_,String>(1)? ,"state":visible_state,"result":r.get::<_,Option<String>>(3)?.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"refusal":r.get::<_,Option<String>>(4)? ,"observationCallId":r.get::<_,Option<String>>(5)? ,"createdAt":r.get::<_,i64>(6)? ,"updatedAt":r.get::<_,i64>(7)?}))
    }) { Ok(v)=>json_response(v),Err(_)=>fail(StatusCode::NOT_FOUND,"Effect receipt is not available.") }
}
async fn settle_effect(
    State(state): State<Konae>,
    AxumPath((project, conversation, call_id)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), BODY_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Effect settlement exceeds 4 MiB.",
            )
        }
    };
    let input: EffectSettlement = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid effect settlement."),
    };
    if input.state != "acknowledged"
        || input.refusal.is_some()
        || input.result.is_some()
        || input.observation_call_id.is_some()
        || input
            .acknowledgement
            .as_ref()
            .is_some_and(|s| s.len() > 512)
    {
        return fail(StatusCode::BAD_REQUEST,"Only an explicit informed acknowledgement can be recorded here; observations must use the authorized MCP read-back.");
    }
    let Some(acknowledgement) = input
        .acknowledgement
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return fail(
            StatusCode::BAD_REQUEST,
            "An informed acknowledgement must name the owner's decision.",
        );
    };
    let now = now_ms();
    // The route can resolve only a completed unknown outcome. It can never
    // release a call whose backend dispatch is still running.
    let changed=match db.execute("UPDATE conversation_effects SET state='acknowledged',refusal=?5,updated_at=?6 WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4 AND state='unknown'",params![owner,project,conversation,call_id,format!("Owner acknowledged unresolved outcome: {}",acknowledgement),now]){Ok(v)=>v,Err(e)=>return internal(e)};
    if changed == 0 {
        return fail(
            StatusCode::CONFLICT,
            "Effect is already settled or unavailable.",
        );
    }
    json_response(json!({"callId":call_id,"state":input.state,"updatedAt":now}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MintId {
    generation: i64,
    candidate: i64,
}
async fn mint_id(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), 4096).await {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::PAYLOAD_TOO_LARGE, "ID request exceeds 4 KiB."),
    };
    let input: MintId = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid native ID reservation."),
    };
    if input.generation < 1
        || input.generation > MAX_SAFE_INTEGER
        || input.candidate < 0
        || input.candidate > MAX_SAFE_INTEGER
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Pi IDs and generation must be safe integers.",
        );
    }
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_execution_leases l JOIN conversations c ON c.id=l.conversation WHERE l.conversation=?1 AND l.owner=?2 AND l.project=?3 AND l.generation=?4 AND l.expires_at>?5 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.format_version=?6 AND c.pi_version=?7 AND c.registry_version=?8)",params![conversation,owner,project,input.generation,now_ms(),SUPPORTED_FORMAT,SUPPORTED_PI_VERSION,SUPPORTED_REGISTRY_VERSION],|r|r.get(0)).unwrap_or(false);
    if !live {
        return fail(
            StatusCode::CONFLICT,
            "Conversation writer generation is stale, unsupported or has no live authority.",
        );
    }
    let old: Option<i64> = tx
        .query_row(
            "SELECT high_water FROM conversation_id_highwater WHERE owner=?1 AND project=?2",
            params![owner, project],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    let next = old.unwrap_or(0).saturating_add(1).max(input.candidate);
    if next <= 0 || next > MAX_SAFE_INTEGER {
        return fail(StatusCode::CONFLICT, "Pi ID namespace exhausted.");
    }
    if let Err(e)=tx.execute("INSERT INTO conversation_id_highwater(owner,project,high_water) VALUES(?1,?2,?3) ON CONFLICT(owner,project) DO UPDATE SET high_water=excluded.high_water",params![owner,project,next]){return internal(e)}
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(json!({"id":next}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitRequest {
    request_id: String,
    expected_seq: i64,
    generation: i64,
    writes: Vec<Vec<Box<RawValue>>>,
    checkpoint: Option<CommitCheckpoint>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitCheckpoint {
    id: String,
    payload: Box<RawValue>,
}
#[derive(Deserialize)]
struct WriteProjection {
    #[serde(rename = "type")]
    kind: String,
    id: Option<i64>,
    value: Option<IdProjection>,
    record: Option<IdProjection>,
    content: Option<Box<RawValue>>,
}
#[derive(Deserialize)]
struct IdProjection {
    id: Option<i64>,
}
#[derive(Deserialize)]
struct BlobProjection {
    blobs: Option<Vec<String>>,
}

async fn commit(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), BODY_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Conversation commit exceeds 4 MiB; upload large immutable files separately.",
            )
        }
    };
    let input: CommitRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid ordered storage commit."),
    };
    if input.request_id.is_empty()
        || input.request_id.len() > 128
        || input.expected_seq < 1
        || input.expected_seq > MAX_SAFE_INTEGER
        || input.generation < 1
        || input.generation > MAX_SAFE_INTEGER
        || input.writes.is_empty()
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Request ID, sequence, generation or ordered writes are invalid.",
        );
    }
    // Hash the exact wire body. RawValue preserves opaque records, including
    // JSON-escaped lone UTF-16 code units, without passing them through Value.
    let content_hash = hex::encode(Sha256::digest(&bytes));
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let current:Option<(i64,i64)>=tx.query_row("SELECT last_seq,current_generation FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND deleted=0",params![conversation,owner,project],|r|Ok((r.get(0)?,r.get(1)?))).optional().unwrap_or(None);
    let Some((last_seq, generation)) = current else {
        return fail(StatusCode::NOT_FOUND, "Conversation is not available.");
    };
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_execution_leases WHERE conversation=?1 AND owner=?2 AND project=?3 AND generation=?4 AND expires_at>?5)",params![conversation,owner,project,input.generation,now_ms()],|r|r.get(0)).unwrap_or(false);
    if !live || generation != input.generation {
        return fail(
            StatusCode::CONFLICT,
            "Conversation writer generation is stale or has no live authority.",
        );
    }
    let prior:Option<(String,String)>=tx.query_row("SELECT content_hash,receipt FROM conversation_batches WHERE conversation=?1 AND request_id=?2 AND owner=?3 AND project=?4",params![conversation,input.request_id,owner,project],|r|Ok((r.get(0)?,r.get(1)?))).optional().unwrap_or(None);
    if let Some((previous, receipt)) = prior {
        if previous != content_hash {
            return fail(
                StatusCode::CONFLICT,
                "Request ID was already used for different conversation content.",
            );
        }
        return match serde_json::from_str::<Value>(&receipt) {
            Ok(v) => json_response(v),
            Err(e) => internal(e),
        };
    }
    if last_seq + 1 != input.expected_seq {
        return fail(StatusCode::CONFLICT,"Conversation sequence changed; read the saved receipt and current sequence before retrying.");
    }
    let now = now_ms();
    let mut accepted = Vec::new();
    let mut refusal: Option<String> = None;
    let mut seq = input.expected_seq - 1;
    let mut total_added = 0_i64;
    let mut highest_id = 0_i64;
    let total = match metadata_bytes(&tx, &owner) {
        Ok(value) => value,
        Err(error) => return internal(error),
    };
    'batches: for (batch_index, batch) in input.writes.iter().enumerate() {
        if batch.is_empty() {
            refusal = Some("Pi StorageWrite batch cannot be empty.".into());
            break;
        }
        let mut encoded = Vec::with_capacity(batch.len());
        for (ordinal, record) in batch.iter().enumerate() {
            let projection: WriteProjection = match serde_json::from_str(record.get()) {
                Ok(value) => value,
                Err(_) => {
                    refusal = Some(
                        "Pi StorageWrite record must name its type and indexed fields.".into(),
                    );
                    break 'batches;
                }
            };
            let kind = projection.kind.as_str();
            let record_id = if kind == "entry" {
                match projection
                    .value
                    .as_ref()
                    .and_then(|v| v.id)
                    .filter(|id| *id >= 0 && *id <= MAX_SAFE_INTEGER)
                {
                    Some(id) => Some(id),
                    None => {
                        refusal = Some("Pi entry IDs must be safe nonnegative integers.".into());
                        break 'batches;
                    }
                }
            } else {
                None
            };
            if kind.starts_with("document.") {
                let document_id = if kind == "document.create" {
                    projection.record.as_ref().and_then(|r| r.id)
                } else {
                    projection.id
                };
                if document_id.is_none_or(|id| id < 0 || id > MAX_SAFE_INTEGER) {
                    refusal = Some(
                        "Pi document StorageWrite must preserve its native safe integer record id."
                            .into(),
                    );
                    break 'batches;
                }
                if matches!(kind, "document.create" | "document.change")
                    && projection.content.is_none()
                {
                    refusal = Some("Pi document StorageWrite must preserve its content.".into());
                    break 'batches;
                }
            }
            let payload = record.get().to_owned();
            let size = payload.len() as i64;
            if kind == "entry" && size > DETAIL_LIMIT as i64 {
                refusal=Some("A native entry exceeds the 1 MiB structured-detail limit; pause and offer an explicit recovery path.".into());
                break 'batches;
            }
            if let Some(id) = record_id {
                let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_used_ids WHERE owner=?1 AND project=?2 AND id=?3)",params![owner,project,id],|r|r.get(0)).unwrap_or(false);
                if exists {
                    refusal = Some(format!("Pi entry ID {id} already exists."));
                    break 'batches;
                }
            }
            encoded.push((ordinal as i64, record_id, kind.to_owned(), payload, size));
        }
        if refusal.is_some() {
            break;
        }
        let batch_bytes: i64 = encoded.iter().map(|r| r.4).sum();
        if total
            .saturating_add(total_added)
            .saturating_add(batch_bytes)
            > OWNER_METADATA_LIMIT
        {
            refusal=Some("Owner conversation metadata quota is 16 MiB. Delete or download conversations before writing more.".into());
            break;
        }
        seq = seq.saturating_add(1);
        if seq > MAX_SAFE_INTEGER {
            refusal = Some("Pi sequence namespace exhausted.".into());
            break;
        }
        if let Err(e) = tx.execute_batch("SAVEPOINT pi_storage_batch") {
            refusal = Some(e.to_string());
            break;
        }
        let batch_start = total_added;
        for (ordinal, record_id, kind, payload, size) in encoded {
            if let Err(e)=tx.execute("INSERT INTO conversation_records(conversation,seq,ordinal,record_id,record_type,format_version,payload,byte_size) VALUES(?1,?2,?3,?4,?5,(SELECT format_version FROM conversations WHERE id=?1),?6,?7)",params![conversation,seq,ordinal,record_id,kind,payload,size]){refusal=Some(if e.to_string().contains("UNIQUE"){"A Pi entry ID already exists.".into()}else{e.to_string()});let _=tx.execute_batch("ROLLBACK TO pi_storage_batch; RELEASE pi_storage_batch");total_added=batch_start;break 'batches}
            total_added += size;
            if let Some(id) = record_id {
                highest_id = highest_id.max(id);
            }
            if let Some(id) = record_id {
                if let Err(e)=tx.execute("INSERT INTO conversation_used_ids(owner,project,id,conversation) VALUES(?1,?2,?3,?4)",params![owner,project,id,conversation]){refusal=Some(if e.to_string().contains("UNIQUE"){format!("Pi entry ID {id} was already allocated in this project.")}else{e.to_string()});let _=tx.execute_batch("ROLLBACK TO pi_storage_batch; RELEASE pi_storage_batch");break 'batches}
            }
        }
        if let Err(e) = tx.execute_batch("RELEASE pi_storage_batch") {
            refusal = Some(e.to_string());
            let _ = tx.execute_batch("ROLLBACK TO pi_storage_batch; RELEASE pi_storage_batch");
            total_added = batch_start;
            break;
        }
        accepted.push(seq);
        let _ = batch_index;
    }
    if accepted.is_empty() {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            refusal.as_deref().unwrap_or("No Pi batches were accepted."),
        );
    }
    if highest_id > 0 {
        if let Err(e)=tx.execute("INSERT INTO conversation_id_highwater(owner,project,high_water) VALUES(?1,?2,?3) ON CONFLICT(owner,project) DO UPDATE SET high_water=max(high_water,excluded.high_water)",params![owner,project,highest_id]){return internal(e)}
    }
    let final_seq = *accepted.last().unwrap_or(&last_seq);
    let mut checkpoint_stored = false;
    let mut checkpoint_bytes_added = 0_i64;
    if refusal.is_none() {
        if let Some(checkpoint) = input.checkpoint {
            if checkpoint.id.is_empty() || checkpoint.id.len() > 128 {
                return fail(StatusCode::BAD_REQUEST, "Atomic checkpoint ID is invalid.");
            }
            let checkpoint_payload = checkpoint.payload.get().to_owned();
            if checkpoint_payload.len() > 256 * 1024 {
                return fail(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Atomic checkpoint metadata exceeds 256 KiB.",
                );
            }
            if total
                .saturating_add(total_added)
                .saturating_add(checkpoint_payload.len() as i64)
                > OWNER_METADATA_LIMIT
            {
                return fail(StatusCode::INSUFFICIENT_STORAGE,"Owner conversation metadata quota is 16 MiB. Delete or download conversations before writing more.");
            }
            let projection: BlobProjection = match serde_json::from_str(checkpoint.payload.get()) {
                Ok(value) => value,
                Err(_) => {
                    return fail(
                        StatusCode::BAD_REQUEST,
                        "Checkpoint blob references are invalid.",
                    )
                }
            };
            let refs = projection.blobs.unwrap_or_default();
            if refs.iter().any(|value| !is_digest(value)) {
                return fail(
                    StatusCode::BAD_REQUEST,
                    "Checkpoint blob references must be SHA-256 digests.",
                );
            }
            for digest in &refs {
                let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM conversation_blob_refs r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND r.digest=?2 AND c.owner=?3 AND c.project=?4 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?3 AND project=?4) AND c.deleted=0)",
                    params![conversation,digest,owner,project],
                    |r| r.get(0),
                )
                .unwrap_or(false);
                if !exists {
                    return fail(
                        StatusCode::CONFLICT,
                        "Checkpoint references an immutable file that has not been uploaded.",
                    );
                }
            }
            if let Err(e)=tx.execute("INSERT INTO conversation_checkpoints(conversation,checkpoint_id,seq,payload,byte_size,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![conversation,checkpoint.id,final_seq,checkpoint_payload,checkpoint_payload.len() as i64,now]){return fail(StatusCode::CONFLICT,&format!("Atomic checkpoint already exists or could not be stored: {e}"))}
            checkpoint_bytes_added = checkpoint_payload.len() as i64;
            for digest in &refs {
                let size: i64 = tx
                    .query_row(
                        "SELECT size FROM conversation_blobs WHERE digest=?1",
                        [digest],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                if let Err(e)=tx.execute("INSERT OR IGNORE INTO conversation_blob_refs(conversation,digest,size) VALUES(?1,?2,?3)",params![conversation,digest,size]){return internal(e)}
            }
            checkpoint_stored = true;
        }
    }
    let receipt = json!({"requestId":input.request_id,"stored":accepted.len(),"sequences":accepted,"lastSeq":final_seq,"refused":refusal,"checkpointStored":checkpoint_stored});
    let receipt_text = serde_json::to_string(&receipt).unwrap();
    if total
        .saturating_add(total_added)
        .saturating_add(checkpoint_bytes_added)
        .saturating_add(receipt_text.len() as i64)
        > OWNER_METADATA_LIMIT
    {
        return fail(StatusCode::INSUFFICIENT_STORAGE,"Owner conversation metadata quota is 16 MiB. Delete or download conversations before writing more.");
    }
    if let Err(e) = tx.execute(
        "UPDATE conversations SET last_seq=?2,updated_at=?3 WHERE id=?1",
        params![conversation, final_seq, now],
    ) {
        return internal(e);
    }
    if let Err(e)=tx.execute("INSERT INTO conversation_batches(conversation,owner,project,request_id,content_hash,expected_seq,generation,receipt,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![conversation,owner,project,input.request_id,content_hash,input.expected_seq,input.generation,receipt_text,now]){return internal(e)}
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(receipt)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordPage {
    before: Option<i64>,
    before_ordinal: Option<i64>,
    limit: Option<usize>,
    ids_only: Option<bool>,
}

/// A bounded, inert reader projection. Raw records stay behind the explicit details route.
fn record_summary(payload: &str) -> Value {
    const TEXT_LIMIT: usize = 1_200;
    let Ok(record) = serde_json::from_str::<Value>(payload) else {
        return json!({"kind":"unreadable"});
    };
    let Some(value) = record.get("value") else {
        return json!({"kind":record.get("type").and_then(Value::as_str).unwrap_or("record")});
    };
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("entry");
    let messages = value.get("model").and_then(Value::as_array).map(|items| items.iter().filter_map(|message| {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("unknown");
        if role == "system" || matches!(kind, "pi.system" | "pi.configuration") { return None; }
        let content = message.get("content").and_then(Value::as_array).map(|parts| parts.iter().take(8).filter_map(|part| {
            if part.get("type").and_then(Value::as_str) != Some("text") { return None; }
            part.get("text").and_then(Value::as_str).map(|text| {
                let clipped = text.chars().take(TEXT_LIMIT).collect::<String>();
                let was_clipped = text.chars().count() > TEXT_LIMIT;
                if was_clipped { json!({"text":clipped,"clipped":true}) } else { json!({"text":clipped}) }
            })
        }).collect::<Vec<_>>()).unwrap_or_default();
        let calls = message.get("content").and_then(Value::as_array).map(|parts| parts.iter().filter(|part| part.get("type").and_then(Value::as_str)==Some("toolCall")).map(|part| {
            let mut call = serde_json::Map::new();
            for key in ["id", "name"] { if let Some(value) = part.get(key) { call.insert(key.to_owned(), value.clone()); } }
            if let Some(timestamp) = message.get("timestamp") { call.insert("timestamp".into(), timestamp.clone()); }
            Value::Object(call)
        }).collect::<Vec<_>>()).unwrap_or_default();
        let string_content = message.get("content").and_then(Value::as_str).map(|text| {
            let clipped = text.chars().take(TEXT_LIMIT).collect::<String>();
            json!([{"text":clipped,"clipped":text.chars().count()>TEXT_LIMIT}])
        });
        let mut summary = serde_json::Map::new();
        summary.insert("role".into(), Value::String(role.to_owned()));
        summary.insert("content".into(), string_content.unwrap_or(Value::Array(content)));
        if !calls.is_empty() { summary.insert("toolCalls".into(), Value::Array(calls)); }
        for key in ["toolCallId", "toolName", "isError", "stopReason", "timestamp"] {
            if let Some(value) = message.get(key).filter(|value| !value.is_null()) { summary.insert(key.to_owned(), value.clone()); }
        }
        Some(Value::Object(summary))
    }).take(8).collect::<Vec<_>>()).unwrap_or_default();
    let data = value.get("data").and_then(Value::as_object);
    let mut summary = json!({"kind":kind,"messages":messages});
    if let Some(intent) = data.and_then(|data| data.get("intent")).and_then(Value::as_object) {
        let mut intent = intent.clone();
        if intent.get("sourceEntryId").and_then(Value::as_str) == Some("$self") {
            let source_id = value.get("id").filter(|id| !id.is_null());
            if let Some(source_id) = source_id {
                intent.insert("sourceEntryId".into(), Value::String(source_id.as_str().map(str::to_owned).unwrap_or_else(|| source_id.to_string())));
            }
        }
        let self_source_resolved = intent.get("sourceEntryId").and_then(Value::as_str) != Some("$self");
        if !intent.is_empty() && self_source_resolved { summary["intent"] = Value::Object(intent); }
    }
    summary
}

async fn records(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    Query(page): Query<RecordPage>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(response) = require_conversation(&db, &owner, &project, &conversation) {
        return response;
    }
    let limit = page.limit.unwrap_or(50);
    if !(1..=50).contains(&limit) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Conversation page limit must be between 1 and 50.",
        );
    }
    let before = page.before.unwrap_or(MAX_SAFE_INTEGER);
    let ids_only = page.ids_only.unwrap_or(false);
    let sql = if ids_only {
        "SELECT r.seq,r.ordinal,r.record_id,r.record_type,r.format_version,r.payload FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND c.owner=?2 AND c.project=?3 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.deleted=0 AND r.record_type='entry' AND (r.seq<?4 OR (r.seq=?4 AND r.ordinal<COALESCE(?5,9223372036854775807))) ORDER BY r.seq DESC,r.ordinal DESC LIMIT ?6"
    } else {
        "SELECT r.seq,r.ordinal,r.record_id,r.record_type,r.format_version,r.payload FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND c.owner=?2 AND c.project=?3 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.deleted=0 AND r.record_type='entry' AND COALESCE(json_extract(r.payload,'$.value.kind'),'') NOT IN ('pi.tool','pi.generation','pi.document.change','pi.system','pi.live','pi.inbox','pi.usage','pi.provider') AND (r.seq<?4 OR (r.seq=?4 AND r.ordinal<COALESCE(?5,9223372036854775807))) ORDER BY r.seq DESC,r.ordinal DESC LIMIT ?6"
    };
    let rows=match db.prepare(sql){Ok(mut q)=>match q.query_map(params![conversation,owner,project,before,page.before_ordinal,limit as i64],|r|{
        let record_id:Option<i64>=r.get(2)?;let record_type:String=r.get(3)?;let payload:String=r.get(5)?;
        Ok(if ids_only { json!({"sequence":r.get::<_,i64>(0)? ,"ordinal":r.get::<_,i64>(1)? ,"id":record_id,"type":record_type,"formatVersion":r.get::<_,i64>(4)?}) } else { json!({"sequence":r.get::<_,i64>(0)? ,"ordinal":r.get::<_,i64>(1)? ,"id":record_id,"type":record_type,"formatVersion":r.get::<_,i64>(4)? ,"summary":record_summary(&payload)}) })
    }){Ok(v)=>match v.collect::<rusqlite::Result<Vec<_>>>(){Ok(v)=>v,Err(e)=>return internal(e)},Err(e)=>return internal(e)},Err(e)=>return internal(e)};
    let next = if rows.len() == limit {
        rows.last()
            .map(|r| json!({"before":r["sequence"],"beforeOrdinal":r["ordinal"]}))
    } else {
        None
    };
    json_response(json!({"items":rows,"nextCursor":next}))
}

fn require_conversation(
    db: &Connection,
    owner: &str,
    project: &str,
    conversation: &str,
) -> Result<(), Response> {
    match db.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations c WHERE c.id=?1 AND c.owner=?2 AND c.project=?3 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.deleted=0)",
        params![conversation, owner, project],
        |r| r.get::<_, bool>(0),
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(fail(StatusCode::NOT_FOUND, "Conversation is not available.")),
        Err(error) => Err(internal(error)),
    }
}

async fn details(
    State(state): State<Konae>,
    AxumPath((project, conversation, record)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let id = match record.parse::<i64>() {
        Ok(v) if v >= 0 && v <= MAX_SAFE_INTEGER => v,
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "Pi entry ID must be a safe integer.",
            )
        }
    };
    let payload:Option<String>=db.query_row("SELECT r.payload FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND r.record_id=?2 AND c.owner=?3 AND c.project=?4 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?3 AND project=?4) AND c.deleted=0",params![conversation,id,owner,project],|r|r.get(0)).optional().unwrap_or(None);
    match payload {
        Some(v) => raw_response(v),
        None => fail(
            StatusCode::NOT_FOUND,
            "Conversation detail is not available.",
        ),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchPage {
    after: Option<i64>,
    limit: Option<usize>,
    max_bytes: Option<usize>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeBatchResponse {
    sequence: i64,
    writes: Vec<Box<RawValue>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeBatchesResponse {
    batches: Vec<NativeBatchResponse>,
    next_cursor: Option<i64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadBlob {
    digest: String,
    base64: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadCheckpoint {
    checkpoint_id: String,
    sequence: i64,
    created_at: i64,
    payload: Box<RawValue>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadEnvelope {
    version: i64,
    format_version: i64,
    pi_version: String,
    registry_version: String,
    conversation_id: String,
    project_id: String,
    title: String,
    records: Vec<Box<RawValue>>,
    checkpoints: Vec<DownloadCheckpoint>,
    effects: Vec<Value>,
    blobs: Vec<DownloadBlob>,
}
async fn batches(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    Query(page): Query<BatchPage>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(response) = require_conversation(&db, &owner, &project, &conversation) {
        return response;
    }
    let limit = page.limit.unwrap_or(50);
    let max_bytes = match page.max_bytes {
        Some(value) if !(1024..=4 * 1024 * 1024).contains(&value) => return fail(StatusCode::BAD_REQUEST, "Native batch response byte bound must be between 1024 and 4194304 bytes."),
        value => value,
    };
    let limit_ceiling = if max_bytes.is_some() { 250 } else { 50 };
    if !(1..=limit_ceiling).contains(&limit) {
        return fail(
            StatusCode::BAD_REQUEST,
            if max_bytes.is_some() { "Bounded native batch page limit must be between 1 and 250." } else { "Native batch page limit must be between 1 and 50." },
        );
    }
    let after = page.after.unwrap_or(0);
    let mut statement=match db.prepare("SELECT DISTINCT r.seq FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND c.owner=?2 AND c.project=?3 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.deleted=0 AND r.seq>?4 ORDER BY r.seq LIMIT ?5"){Ok(v)=>v,Err(e)=>return internal(e)};
    let sequences = match statement.query_map(
        params![conversation, owner, project, after, limit as i64],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(e) => return internal(e),
        },
        Err(e) => return internal(e),
    };
    let mut grouped: Vec<NativeBatchResponse> = Vec::new();
    let mut page_bytes = 128usize;
    for sequence in sequences {
        let mut q=match db.prepare("SELECT payload FROM conversation_records WHERE conversation=?1 AND seq=?2 ORDER BY ordinal"){Ok(v)=>v,Err(e)=>return internal(e)};
        let writes = match q.query_map(params![conversation, sequence], |r| {
            let payload: String = r.get(0)?;
            RawValue::from_string(payload).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        }) {
            Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() {
                Ok(v) => v,
                Err(e) => return internal(e),
            },
            Err(e) => return internal(e),
        };
        let batch = NativeBatchResponse { sequence, writes };
        if let Some(max_bytes) = max_bytes {
            let batch_bytes = match serde_json::to_vec(&batch) { Ok(value) => value.len(), Err(error) => return internal(error) };
            // Keep a native sequence atomic for replay. A single sequence can
            // exceed the soft page target, but no additional sequence joins it.
            if !grouped.is_empty() && page_bytes + batch_bytes + 1 > max_bytes { break; }
            page_bytes = page_bytes.saturating_add(batch_bytes + 1);
        }
        grouped.push(batch);
    }
    let next_cursor = if let Some(last) = grouped.last() {
        let has_more = match db.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_records r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND c.owner=?2 AND c.project=?3 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND c.deleted=0 AND r.seq>?4)",
            params![conversation, owner, project, last.sequence],
            |row| row.get::<_, bool>(0),
        ) {
            Ok(value) => value,
            Err(error) => return internal(error),
        };
        if has_more { Some(last.sequence) } else { None }
    } else { None };
    match serde_json::to_string(&NativeBatchesResponse {
        batches: grouped,
        next_cursor,
    }) {
        Ok(body) => raw_response(body),
        Err(error) => internal(error),
    }
}

async fn receipt(
    State(state): State<Konae>,
    AxumPath((project, conversation, request_id)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(response) = require_conversation(&db, &owner, &project, &conversation) {
        return response;
    }
    // Receipts are owner-readable after lease loss so a new executor can
    // reconcile an uncertain commit without reviving the old writer.
    match db.query_row("SELECT receipt FROM conversation_batches WHERE conversation=?1 AND owner=?2 AND project=?3 AND request_id=?4",params![conversation,owner,project,request_id],|r|r.get::<_,String>(0)).optional(){Ok(Some(v))=>match serde_json::from_str(&v){Ok(v)=>json_response(v),Err(e)=>internal(e)},Ok(None)=>fail(StatusCode::NOT_FOUND,"Conversation receipt is not available."),Err(e)=>internal(e)}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckpointInput {
    checkpoint_id: String,
    sequence: i64,
    payload: Box<RawValue>,
}
async fn checkpoint(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes = match to_bytes(request.into_body(), 256 * 1024).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Checkpoint metadata exceeds 256 KiB.",
            )
        }
    };
    let input: CheckpointInput = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "Invalid immutable checkpoint."),
    };
    if input.checkpoint_id.is_empty()
        || input.checkpoint_id.len() > 128
        || input.sequence < 0
        || input.sequence > MAX_SAFE_INTEGER
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "Checkpoint ID or sequence is invalid.",
        );
    }
    let encoded = input.payload.get().to_owned();
    let projection: BlobProjection = match serde_json::from_str(input.payload.get()) {
        Ok(value) => value,
        Err(_) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "Checkpoint blob references are invalid.",
            )
        }
    };
    let refs = projection.blobs.unwrap_or_default();
    if refs.iter().any(|value| !is_digest(value)) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Checkpoint blob references must be SHA-256 digests.",
        );
    }
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_live_authority l JOIN conversations c ON c.id=l.conversation WHERE l.conversation=?1 AND l.owner=?2 AND l.project=?3 AND l.generation=c.current_generation AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3))",params![conversation,owner,project],|r|r.get(0)).unwrap_or(false);
    if !live {
        return fail(
            StatusCode::CONFLICT,
            "Checkpoint requires the current live writer generation.",
        );
    }
    let checkpoint_seq:Option<i64>=tx.query_row("SELECT last_seq FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND deleted=0",params![conversation,owner,project],|r|r.get(0)).optional().unwrap_or(None);
    if checkpoint_seq.is_none_or(|last| input.sequence > last) {
        return fail(
            StatusCode::CONFLICT,
            "Checkpoint sequence is ahead of acknowledged native history.",
        );
    }
    for digest in &refs {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_blob_refs r JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND r.digest=?2 AND c.owner=?3 AND c.project=?4 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?3 AND project=?4) AND c.deleted=0)",
                params![conversation,digest,owner,project],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !exists {
            return fail(
                StatusCode::CONFLICT,
                "Checkpoint references an immutable file that has not been uploaded.",
            );
        }
    }
    let total = match metadata_bytes(&tx, &owner) {
        Ok(value) => value,
        Err(error) => return internal(error),
    };
    if total.saturating_add(encoded.len() as i64) > OWNER_METADATA_LIMIT {
        return fail(StatusCode::INSUFFICIENT_STORAGE,"Owner conversation metadata quota is 16 MiB. Delete or download conversations before storing this checkpoint.");
    }
    if let Err(e)=tx.execute("INSERT INTO conversation_checkpoints(conversation,checkpoint_id,seq,payload,byte_size,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![conversation,input.checkpoint_id,input.sequence,encoded,bytes.len() as i64,now_ms()]){return fail(StatusCode::CONFLICT,&format!("Checkpoint is immutable and already exists or could not be stored: {e}"))}
    for digest in &refs {
        let size: i64 = tx
            .query_row(
                "SELECT size FROM conversation_blobs WHERE digest=?1",
                [digest],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if let Err(e)=tx.execute("INSERT OR IGNORE INTO conversation_blob_refs(conversation,digest,size) VALUES(?1,?2,?3)",params![conversation,digest,size]){return internal(e)}
    }
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(
        json!({"checkpointId":input.checkpoint_id,"sequence":input.sequence,"stored":true}),
    )
}

#[derive(Deserialize)]
struct CheckpointQuery { summary: Option<bool>, latest: Option<bool>, checkpoint_id: Option<String> }
async fn checkpoints(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    Query(query): Query<CheckpointQuery>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(response) = require_conversation(&db, &owner, &project, &conversation) {
        return response;
    }
    if query.summary.unwrap_or(false) {
        let latest: Option<(String, i64, String, i64)> = db.query_row(
            "SELECT checkpoint_id,seq,payload,created_at FROM conversation_checkpoints WHERE conversation=?1 ORDER BY seq DESC,checkpoint_id DESC LIMIT 1",
            [&conversation],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional().unwrap_or(None);
        let item = latest.and_then(|(checkpoint_id, sequence, payload, created_at)| {
            let value = serde_json::from_str::<Value>(&payload).ok()?;
            Some(json!({"checkpointId":checkpoint_id,"sequence":sequence,"createdAt":created_at,"status":value.get("status"),"sourcesRevision":value.get("sourcesRevision"),"candidateId":value.get("candidateId"),"inspectionOnly":value.pointer("/lifecycle/inspectionOnly")}))
        });
        return json_response(json!({"item":item}));
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Item {
        checkpoint_id: String,
        sequence: i64,
        payload: Box<RawValue>,
        created_at: i64,
    }
    #[derive(Serialize)]
    struct Page {
        items: Vec<Item>,
    }
    let single = query.latest.unwrap_or(false) || query.checkpoint_id.is_some();
    let mut q=match db.prepare("SELECT p.checkpoint_id,p.seq,p.payload,p.created_at FROM conversation_checkpoints p JOIN conversations c ON c.id=p.conversation WHERE p.conversation=?1 AND c.owner=?2 AND c.project=?3 AND c.deleted=0 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND (?4 IS NULL OR p.checkpoint_id=?4) ORDER BY p.seq DESC,p.checkpoint_id DESC LIMIT CASE WHEN ?5 THEN 1 ELSE 50 END"){Ok(v)=>v,Err(e)=>return internal(e)};
    let items = match q.query_map(params![conversation, owner, project, query.checkpoint_id.as_deref(), single], |r| {
        let payload: String = r.get(2)?;
        let payload = RawValue::from_string(payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(Item {
            checkpoint_id: r.get(0)?,
            sequence: r.get(1)?,
            payload,
            created_at: r.get(3)?,
        })
    }) {
        Ok(rows) => match rows.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(e) => return internal(e),
        },
        Err(e) => return internal(e),
    };
    match serde_json::to_string(&Page { items }) {
        Ok(body) => raw_response(body),
        Err(e) => internal(e),
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
async fn put_blob(
    State(state): State<Konae>,
    AxumPath((project, conversation, digest)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !is_digest(&digest) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Blob path must be a SHA-256 digest.",
        );
    }
    let bytes = match to_bytes(request.into_body(), BLOB_LIMIT).await {
        Ok(v) => v,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Immutable file exceeds 20 MiB.",
            )
        }
    };
    if hex::encode(Sha256::digest(&bytes)) != digest.to_lowercase() {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Uploaded immutable file does not match its SHA-256 digest.",
        );
    }
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversations c WHERE c.id=?1 AND c.owner=?2 AND c.project=?3 AND c.deleted=0 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3))",params![conversation,owner,project],|r|r.get(0)).unwrap_or(false);
    if !current {
        return fail(StatusCode::NOT_FOUND, "Conversation is not available.");
    }
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT bytes FROM conversation_blobs WHERE digest=?1",
            [&digest],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    let owner_has_digest:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_blob_refs r JOIN conversations c ON c.id=r.conversation WHERE r.digest=?1 AND c.owner=?2)",params![digest,owner],|r|r.get(0)).unwrap_or(false);
    if !owner_has_digest {
        let total:i64=tx.query_row("SELECT COALESCE(SUM(size),0) FROM (SELECT DISTINCT b.digest,b.size FROM conversation_blobs b JOIN conversation_blob_refs r ON r.digest=b.digest JOIN conversations c ON c.id=r.conversation WHERE c.owner=?1)",[&owner],|r|r.get(0)).unwrap_or(0);
        if total.saturating_add(bytes.len() as i64) > OWNER_BLOB_LIMIT {
            return fail(StatusCode::INSUFFICIENT_STORAGE,"Owner conversation file quota is 64 MiB. Delete or download conversations before uploading more.");
        }
    }
    if let Some(old) = existing {
        if old != bytes.as_ref() {
            return fail(
                StatusCode::CONFLICT,
                "Content-addressed immutable blob digest collision.",
            );
        }
    } else {
        if let Err(e) = tx.execute(
            "INSERT INTO conversation_blobs(digest,bytes,size) VALUES(?1,?2,?3)",
            params![digest, bytes.as_ref(), bytes.len() as i64],
        ) {
            return internal(e);
        }
    }
    if let Err(e) = tx.execute(
        "INSERT OR IGNORE INTO conversation_blob_refs(conversation,digest,size) VALUES(?1,?2,?3)",
        params![conversation, digest, bytes.len() as i64],
    ) {
        return internal(e);
    }
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(json!({"digest":digest,"size":bytes.len(),"stored":true}))
}

async fn blob(
    State(state): State<Konae>,
    AxumPath((project, conversation, digest)): AxumPath<(String, String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let bytes:Option<Vec<u8>>=db.query_row("SELECT b.bytes FROM conversation_blobs b JOIN conversation_blob_refs r ON r.digest=b.digest JOIN conversations c ON c.id=r.conversation WHERE r.conversation=?1 AND r.digest=?2 AND c.owner=?3 AND c.project=?4 AND c.deleted=0 AND c.project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?3 AND project=?4)",params![conversation,digest,owner,project],|r|r.get(0)).optional().unwrap_or(None);
    match bytes {
        Some(bytes) => {
            let mut response = bytes.into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                "application/octet-stream".parse().unwrap(),
            );
            response
        }
        None => fail(
            StatusCode::NOT_FOUND,
            "Immutable conversation file is not available.",
        ),
    }
}

async fn download(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let record:Option<(i64,String,String,String,String)>=db.query_row("SELECT format_version,pi_version,registry_version,title,project FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3) AND deleted=0",params![conversation,owner,project],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().unwrap_or(None);
    let Some((format, pi, registry, title, project_id)) = record else {
        return fail(StatusCode::NOT_FOUND, "Conversation is not available.");
    };
    let mut q = match db.prepare(
        "SELECT payload FROM conversation_records WHERE conversation=?1 ORDER BY seq,ordinal",
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let records = match q.query_map([&conversation], |r| {
        let payload: String = r.get(0)?;
        RawValue::from_string(payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    }) {
        Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(e) => return internal(e),
        },
        Err(e) => return internal(e),
    };
    let mut q=match db.prepare("SELECT checkpoint_id,seq,payload,created_at FROM conversation_checkpoints WHERE conversation=?1 ORDER BY seq,checkpoint_id"){Ok(v)=>v,Err(e)=>return internal(e)};
    let checkpoints = match q.query_map([&conversation], |r| {
        let payload: String = r.get(2)?;
        let payload = RawValue::from_string(payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(DownloadCheckpoint { checkpoint_id: r.get(0)?, sequence: r.get(1)?, payload, created_at: r.get(3)? })
    }) {
        Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(e) => return internal(e),
        },
        Err(e) => return internal(e),
    };
    let mut q=match db.prepare("SELECT call_id,executor_id,generation,tool,effect_class,args_hash,intent,state,result,refusal,observation_call_id,created_at,updated_at FROM conversation_effects WHERE owner=?1 AND project=?2 AND conversation=?3 ORDER BY created_at,call_id"){Ok(v)=>v,Err(e)=>return internal(e)};
    let effects = match q.query_map(params![owner,project,conversation], |r| {
        let intent: Option<String> = r.get(6)?;
        let result: Option<String> = r.get(8)?;
        Ok(json!({"callId":r.get::<_,String>(0)?,"executorId":r.get::<_,String>(1)?,"generation":r.get::<_,i64>(2)?,"tool":r.get::<_,String>(3)?,"effectClass":r.get::<_,String>(4)?,"argsHash":r.get::<_,String>(5)?,"intent":intent.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"state":r.get::<_,String>(7)?,"result":result.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"refusal":r.get::<_,Option<String>>(9)?,"observationCallId":r.get::<_,Option<String>>(10)?,"createdAt":r.get::<_,i64>(11)?,"updatedAt":r.get::<_,i64>(12)?}))
    }) {
        Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() { Ok(v) => v, Err(e) => return internal(e) },
        Err(e) => return internal(e),
    };
    let mut q=match db.prepare("SELECT b.digest,b.bytes FROM conversation_blob_refs r JOIN conversation_blobs b ON b.digest=r.digest WHERE r.conversation=?1 ORDER BY b.digest"){Ok(v)=>v,Err(e)=>return internal(e)};
    let file_values = match q.query_map([&conversation], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    }) {
        Ok(v) => match v.collect::<rusqlite::Result<Vec<_>>>() {
            Ok(v) => v,
            Err(e) => return internal(e),
        },
        Err(e) => return internal(e),
    };
    let total: usize = file_values.iter().map(|(_, b)| b.len()).sum();
    if total > OWNER_BLOB_LIMIT as usize {
        return fail(
            StatusCode::INSUFFICIENT_STORAGE,
            "Conversation download exceeds the 64 MiB file limit.",
        );
    }
    let blobs = file_values
        .into_iter()
        .map(|(digest, bytes)| DownloadBlob {
            digest,
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
        .collect();
    let envelope = DownloadEnvelope {
        version: 1,
        format_version: format,
        pi_version: pi,
        registry_version: registry,
        conversation_id: conversation,
        project_id,
        title,
        records,
        checkpoints,
        effects,
        blobs,
    };
    let bytes = match serde_json::to_vec(&envelope) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if bytes.len() > 112 * 1024 * 1024 {
        return fail(
            StatusCode::INSUFFICIENT_STORAGE,
            "Versioned conversation download exceeds 112 MiB, the encoded bound for 64 MiB of files plus the 16 MiB metadata allowance.",
        );
    }
    raw_response(String::from_utf8(bytes).expect("serde_json emits UTF-8"))
}

async fn remove(
    State(state): State<Konae>,
    AxumPath((project, conversation)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let (owner, mut db) = match owner_request(&state, request.headers().clone(), &project).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let tx = match db.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let found:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1 AND owner=?2 AND project=?3 AND project_epoch=(SELECT epoch FROM conversation_project_epochs WHERE owner=?2 AND project=?3))",params![conversation,owner,project],|r|r.get(0)).unwrap_or(false);
    if !found {
        return fail(StatusCode::NOT_FOUND, "Conversation is not available.");
    }
    let unresolved: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_effects WHERE owner=?1 AND project=?2 AND conversation=?3 AND state IN ('running','unknown'))",
            params![owner, project, conversation],
            |r| r.get(0),
        )
        .unwrap_or(true);
    if unresolved {
        return fail(
            StatusCode::CONFLICT,
            "This conversation has a running or unresolved external effect. Read it back through its documented MCP rule before deleting the conversation.",
        );
    }
    if let Err(e)=tx.execute("INSERT OR REPLACE INTO conversation_tombstones(owner,project,conversation,deleted_at) VALUES(?1,?2,?3,?4)",params![owner,project,conversation,now_ms()]){return internal(e)}
    for table in [
        "conversation_live_authority",
        "conversation_effects",
        "conversation_batches",
        "conversation_records",
        "conversation_checkpoints",
        "conversation_blob_refs",
    ] {
        if let Err(e) = tx.execute(
            &format!("DELETE FROM {table} WHERE conversation=?1"),
            [&conversation],
        ) {
            return internal(e);
        }
    }
    if let Err(e) = tx.execute("DELETE FROM conversations WHERE id=?1", [&conversation]) {
        return internal(e);
    }
    let _ = tx.execute("DELETE FROM conversation_execution_leases WHERE owner=?1 AND project=?2 AND conversation=?3",params![owner,project,conversation]);
    let _ = tx.execute(
        "DELETE FROM conversation_stop_requests WHERE owner=?1 AND project=?2 AND conversation=?3",
        params![owner, project, conversation],
    );
    if let Err(e)=tx.execute("DELETE FROM conversation_blobs WHERE NOT EXISTS(SELECT 1 FROM conversation_blob_refs r WHERE r.digest=conversation_blobs.digest)",[]){return internal(e)}
    if let Err(e) = tx.commit() {
        return internal(e);
    }
    json_response(json!({"deleted":true,"conversationId":conversation}))
}

pub(super) fn delete_project_tx(
    tx: &rusqlite::Transaction<'_>,
    owner: &str,
    project: &str,
    now: i64,
) -> rusqlite::Result<()> {
    schema(tx)?;
    tx.execute(
        "INSERT OR IGNORE INTO conversation_project_epochs(owner,project,epoch) VALUES(?1,?2,1)",
        params![owner, project],
    )?;
    tx.execute("UPDATE conversation_project_epochs SET epoch=epoch+1 WHERE owner=?1 AND project=?2 AND epoch<?3",params![owner,project,MAX_SAFE_INTEGER])?;
    tx.execute("INSERT OR IGNORE INTO conversation_project_generations(owner,project,generation) VALUES(?1,?2,0)",params![owner,project])?;
    tx.execute("UPDATE conversation_project_generations SET generation=generation+1 WHERE owner=?1 AND project=?2 AND generation<?3",params![owner,project,MAX_SAFE_INTEGER])?;
    tx.execute("INSERT OR IGNORE INTO conversation_tombstones(owner,project,conversation,deleted_at) SELECT owner,project,id,?3 FROM conversations WHERE owner=?1 AND project=?2",params![owner,project,now])?;
    for table in [
        "conversation_live_authority",
        "conversation_batches",
        "conversation_records",
        "conversation_checkpoints",
        "conversation_blob_refs",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE conversation IN (SELECT id FROM conversations WHERE owner=?1 AND project=?2)"),params![owner,project])?;
    }
    tx.execute(
        "DELETE FROM conversations WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute(
        "DELETE FROM conversation_execution_leases WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute(
        "DELETE FROM conversation_stop_requests WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute(
        "DELETE FROM conversation_effects WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute(
        "DELETE FROM conversation_used_ids WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute(
        "DELETE FROM conversation_id_highwater WHERE owner=?1 AND project=?2",
        params![owner, project],
    )?;
    tx.execute("DELETE FROM conversation_blobs WHERE NOT EXISTS(SELECT 1 FROM conversation_blob_refs r WHERE r.digest=conversation_blobs.digest)",[])?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

/// Validate browser execution identity and durably admit an MCP effect before
/// the tool dispatcher is entered. The transaction ends before remote work.
pub(super) fn authorize_agent_mcp_call(
    state: &Konae,
    owner: &str,
    project: &str,
    conversation: &str,
    executor: &str,
    generation: i64,
    call_id: &str,
    tool: &str,
    args: &Value,
    observes: Option<&str>,
) -> Result<bool, String> {
    if call_id.is_empty() || call_id.len() > 128 || executor.is_empty() || executor.len() > 128 {
        return Err("Agent execution identity is invalid.".into());
    }
    let mut db = super::mcp::store(state).map_err(|e| e.to_string())?;
    schema(&db).map_err(|e| e.to_string())?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let now = now_ms();
    let bound_project = if let Some(id) = args["projectId"].as_str() {
        Some(id.to_owned())
    } else if tool == "release_apply" {
        let plan = args["planId"].as_str().unwrap_or_default();
        tx.query_row("SELECT project FROM plans WHERE id=?1", [plan], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
    } else {
        None
    };
    if bound_project.as_deref().is_some_and(|id| id != project) {
        return Err(
            "Agent MCP call refused: its project binding does not match the leased project.".into(),
        );
    }
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_execution_leases WHERE owner=?1 AND project=?2 AND conversation=?3 AND executor_id=?4 AND generation=?5 AND expires_at>?6)",params![owner,project,conversation,executor,generation,now],|r|r.get(0)).unwrap_or(false);
    let is_write = agent_tool_writes(tool, args);
    let observed_write: Option<(String, String)> = if let Some(original) = observes {
        tx.query_row(
            "SELECT tool,state FROM conversation_effects WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4 AND state IN ('unknown','running')",
            params![owner, project, conversation, original],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
    } else {
        None
    };
    let authorized_observation = if let (Some(original), Some((original_tool, state))) = (observes, observed_write.as_ref()) {
        let documented_read = super::guide::READ_BACK
            .iter()
            .find(|(write, _)| *write == original_tool)
            .map(|(_, read)| *read);
        let read_project = args["projectId"].as_str();
        if documented_read != Some(tool)
            || is_write
            || read_project.is_some_and(|id| id != project)
            || (read_tool_requires_project(tool) && read_project != Some(project))
        {
            return Err("This is not the documented read-back for the selected effect and project.".into());
        }
        if original_tool == "database_execute"
            && (args["operation"].as_str() != Some("sql")
                || !agent_sql_is_bounded_read(args["input"]["sql"].as_str().unwrap_or("")))
        {
            return Err("A database effect read-back needs one bounded, read-only SELECT against this project.".into());
        }
        if state == "running" {
            if active_agent_effects()
                .lock()
                .map_err(|_| "Agent effect registry is unavailable.".to_owned())?
                .contains(&effect_key(owner, project, original))
            {
                return Err("The original MCP write is still running; read-back is not available yet.".into());
            }
            tx.execute("UPDATE conversation_effects SET state='unknown',updated_at=?5 WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4 AND state='running'",params![owner,project,conversation,original,now]).map_err(|e|e.to_string())?;
        }
        true
    } else {
        false
    };
    if observes.is_some() && !authorized_observation {
        return Err("The original effect is not awaiting its documented read-back.".into());
    }
    if !live && !authorized_observation {
        return Err("Agent MCP call refused: project execution lease is expired or fenced.".into());
    }
    if is_write {
        let blocked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_effects WHERE owner=?1 AND project=?2 AND state IN ('running','unknown'))",params![owner,project],|r|r.get(0)).unwrap_or(true);
        if blocked {
            return Err("Agent MCP write refused: a previous effect is still running or has an unknown outcome; reconcile it before continuing.".into());
        }
        let canonical = serde_json::to_vec(args).map_err(|e| e.to_string())?;
        let hash = hex::encode(Sha256::digest(&canonical));
        let inserted=tx.execute("INSERT INTO conversation_effects(owner,project,conversation,call_id,executor_id,generation,tool,effect_class,args_hash,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'running',?10,?10)",params![owner,project,conversation,call_id,executor,generation,tool,if tool=="release_apply" {"publish"} else {"write"},hash,now]).map_err(|e|e.to_string())?;
        if inserted != 1 {
            return Err("Agent MCP call ID was already admitted; inspect its receipt before taking another action.".into());
        }
        if metadata_bytes(&tx, owner).map_err(|e| e.to_string())? > OWNER_METADATA_LIMIT {
            return Err("Project effect receipts reached the owner's conversation metadata quota; no MCP write was dispatched.".into());
        }
        active_agent_effects()
            .lock()
            .map_err(|_| "Agent effect registry is unavailable.".to_owned())?
            .insert(effect_key(owner, project, call_id));
    }
    if let Err(error) = tx.commit() {
        if is_write {
            if let Ok(mut active) = active_agent_effects().lock() {
                active.remove(&effect_key(owner, project, call_id));
            }
        }
        return Err(error.to_string());
    }
    Ok(is_write)
}

pub(super) fn finish_agent_mcp_call(
    state: &Konae,
    owner: &str,
    project: &str,
    conversation: &str,
    call_id: &str,
    write: bool,
    success: bool,
    definitive_refusal: bool,
    observation_of: Option<&str>,
    tool: &str,
) -> Result<(), String> {
    let now = now_ms();
    if write {
        let status = if definitive_refusal {
            "refused"
        } else {
            "unknown"
        };
        let refusal = definitive_refusal.then(|| {
            if tool == "project_sources_save" {
                "Source revision compare-and-swap refused this write before mutation."
            } else {
                "The platform definitively refused this write before mutation."
            }
        });
        let persisted = super::mcp::store(state).and_then(|db| {
            db.execute("UPDATE conversation_effects SET state=?5,refusal=?6,updated_at=?7 WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4 AND state='running'",params![owner,project,conversation,call_id,status,refusal,now])?;
            Ok(())
        });
        if let Ok(mut active) = active_agent_effects().lock() {
            active.remove(&effect_key(owner, project, call_id));
        }
        persisted.map_err(|e| e.to_string())?;
    }
    if success {
        if let Some(original) = observation_of {
            let db = super::mcp::store(state).map_err(|e| e.to_string())?;
            if !readback_matches(state, owner, project, original, tool)? {
                return Err(
                    "This read is not the documented read-back for the pending effect.".into(),
                );
            }
            let changed=db.execute("UPDATE conversation_effects SET state='observed',observation_call_id=?5,updated_at=?6 WHERE owner=?1 AND project=?2 AND conversation=?3 AND call_id=?4 AND state='unknown'",params![owner,project,conversation,original,call_id,now]).map_err(|e|e.to_string())?;
            if changed != 1 {
                return Err("The original write is not awaiting read-back.".into());
            }
        }
    }
    Ok(())
}

fn effect_key(owner: &str, project: &str, call_id: &str) -> String {
    format!("{owner}\u{1f}{project}\u{1f}{call_id}")
}

// The caller holds the same project lock used for MCP admission until deletion ends.
pub(super) fn ensure_no_running_project_effect(owner: &str, project: &str) -> Result<(), String> {
    let prefix = effect_key(owner, project, "");
    let active = active_agent_effects().lock().map_err(|_| "Agent effect registry is unavailable.".to_owned())?;
    if active.iter().any(|key| key.starts_with(&prefix)) {
        return Err("This project still has a running agent write. Wait for it to finish before deleting the project.".into());
    }
    Ok(())
}

fn read_tool_requires_project(tool: &str) -> bool {
    !matches!(tool, "auth_session" | "mail_status" | "projects_list" | "tokens_list" | "transfer_status" | "transfer_read")
}
fn active_agent_effects() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static ACTIVE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    ACTIVE.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn readback_matches(
    _state: &Konae,
    owner: &str,
    project: &str,
    call_id: &str,
    read_tool: &str,
) -> Result<bool, String> {
    let db = super::mcp::store(_state).map_err(|e| e.to_string())?;
    let write: Option<String> = db
        .query_row(
            "SELECT tool FROM conversation_effects WHERE owner=?1 AND project=?2 AND call_id=?3",
            params![owner, project, call_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(write.as_deref().and_then(|name| {
        super::guide::READ_BACK
            .iter()
            .find(|(write, _)| *write == name)
            .map(|(_, read)| *read)
    }) == Some(read_tool))
}

fn agent_tool_writes(tool: &str, args: &Value) -> bool {
    if tool == "database_execute" {
        let operation = args["operation"].as_str().unwrap_or("");
        if matches!(operation, "inspect" | "browse" | "list-backups") {
            return false;
        }
        if operation == "sql" {
            return !agent_sql_is_read_only(args["input"]["sql"].as_str().unwrap_or(""));
        }
        return true;
    }
    !matches!(
        tool,
        "session_inspect"
            | "platform_guide"
            | "project_inspect"
            | "project_sources_read"
            | "usage_summary"
            | "notices_read"
            | "app_users_list"
            | "domain_read"
            | "path_read"
            | "modules_read"
            | "app_invitations_list"
            | "hostname_read"
    )
}

pub(super) fn agent_sql_is_read_only(sql: &str) -> bool {
    let statement = sql.trim().strip_suffix(';').unwrap_or(sql.trim()).trim();
    let lower = statement.to_ascii_lowercase();
    let select = lower
        .strip_prefix("select")
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace);
    select && !statement.contains(';')
}

fn agent_sql_is_bounded_read(sql: &str) -> bool {
    if !agent_sql_is_read_only(sql) {
        return false;
    }
    let statement = sql.trim().strip_suffix(';').unwrap_or(sql.trim()).trim();
    let tokens = sql_tokens(statement);
    let top: Vec<&SqlTok> = tokens.iter().filter(|token| token.depth == 0).collect();
    let Some(index) = limit_index(&top) else { return false };
    let is_integer = |token: &SqlTok| {
        token.kind == SqlTokKind::Symbol && token.text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let limit = match &top[index + 1..] {
        [limit] => limit,
        [limit, offset, rows] if offset.is_word("OFFSET") && is_integer(rows) => limit,
        _ => return false,
    };
    is_integer(limit) && limit.text.parse::<u32>().is_ok_and(|rows| (1..=1000).contains(&rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    #[test]
    fn summary_projects_owner_text_and_text_parts_but_hides_system_and_caps_content() {
        let exact = "owner request";
        let payload = json!({"type":"entry","value":{"id":1,"kind":"konae.owner-message","model":[
            {"role":"user","content":exact,"timestamp":123},
            {"role":"assistant","content":[{"type":"text","text":"visible answer"},{"type":"thinking","thinking":"private internal text"}],"stopReason":"stop"},
            {"role":"system","content":"private configuration"}
        ]}}).to_string();
        let summary = record_summary(&payload);
        assert_eq!(summary["kind"], "konae.owner-message");
        assert_eq!(summary["messages"].as_array().unwrap().len(), 2);
        assert_eq!(summary["messages"][0]["content"][0]["text"], exact);
        assert_eq!(summary["messages"][1]["content"][0]["text"], "visible answer");
        assert_eq!(summary["messages"][1]["stopReason"], "stop");
        assert!(!summary.to_string().contains("private internal text"));
        assert!(!summary.to_string().contains("private configuration"));
        let long = "x".repeat(1_300);
        let clipped = record_summary(&json!({"type":"entry","value":{"kind":"pi.user","model":[{"role":"user","content":long}]}}).to_string());
        assert_eq!(clipped["messages"][0]["content"][0]["text"].as_str().unwrap().chars().count(), 1_200);
        assert_eq!(clipped["messages"][0]["content"][0]["clipped"], true);
    }

    #[test]
    fn summary_resolves_owner_intent_self_source_to_the_actual_entry_id_without_running_it() {
        let payload = json!({"type":"entry","value":{"id":"entry-actual-17","kind":"konae.owner-message","data":{"intent":{"origin":"owner-message","sourceEntryId":"$self","text":"Keep the app portable","declaredAt":1700000000123_i64}},"model":[{"role":"user","content":"Objective: Keep the app portable\n\nBuild a checklist.","timestamp":1700000000124_i64}]}}).to_string();
        let summary = record_summary(&payload);
        assert_eq!(summary["intent"], json!({"origin":"owner-message","sourceEntryId":"entry-actual-17","text":"Keep the app portable","declaredAt":1700000000123_i64}));
        assert_eq!(summary["messages"][0]["content"][0]["text"], "Objective: Keep the app portable\n\nBuild a checklist.");

        let ordinary = json!({"type":"entry","value":{"id":"entry-actual-18","kind":"konae.owner-message","model":[{"role":"user","content":"Build a checklist."}]}}).to_string();
        assert!(record_summary(&ordinary).get("intent").is_none());
    }

    async fn fixture() -> (
        PathBuf,
        AppState,
        Konae,
        kernel::auth::AuthSession,
        kernel::auth::AuthSession,
    ) {
        let root =
            std::env::temp_dir().join(format!("konae-conversation-routes-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = AppState::for_test(&root);
        let konae = Konae::for_test(state.clone());
        let auth = super::super::owners::creator_store(&konae).unwrap();
        let alice = auth
            .signup("alice@example.test", "fixture-A-password-2026")
            .unwrap();
        let bob = auth
            .signup("bob@example.test", "fixture-B-password-2026")
            .unwrap();
        let db = super::super::mcp::store(&konae).unwrap();
        db.execute(
            "INSERT INTO projects(id,owner,body) VALUES('project_alice',?1,'{}')",
            [&alice.user.user_id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO projects(id,owner,body) VALUES('project_bob',?1,'{}')",
            [&bob.user.user_id],
        )
        .unwrap();
        (root, state, konae, alice, bob)
    }

    async fn call(
        state: &AppState,
        konae: &Konae,
        session: &kernel::auth::AuthSession,
        owner: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        extra: Option<(&str, &str)>,
    ) -> Response {
        let authority = url::Url::parse(&state.public_origin).unwrap();
        let host = authority.host_str().unwrap();
        let host = authority
            .port()
            .map(|p| format!("{host}:{p}"))
            .unwrap_or_else(|| host.to_owned());
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header(header::ORIGIN, &state.public_origin)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!(
                    "{}={}",
                    super::super::mcp::owner_cookie_name(konae),
                    session.token
                ),
            )
            .header("x-konae-owner-id", owner);
        if let Some((name, value)) = extra {
            builder = builder.header(name, value);
        }
        let payload = body
            .map(|v| Body::from(v.to_string()))
            .unwrap_or_else(Body::empty);
        crate::stack(state.clone(), crate::owner_routes(konae.clone()))
            .oneshot(builder.body(payload).unwrap())
            .await
            .unwrap()
    }

    async fn call_raw(
        state: &AppState,
        konae: &Konae,
        session: &kernel::auth::AuthSession,
        owner: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> Response {
        let authority = url::Url::parse(&state.public_origin).unwrap();
        let host = authority.host_str().unwrap();
        let host = authority
            .port()
            .map(|p| format!("{host}:{p}"))
            .unwrap_or_else(|| host.to_owned());
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header(header::ORIGIN, &state.public_origin)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!(
                    "{}={}",
                    super::super::mcp::owner_cookie_name(konae),
                    session.token
                ),
            )
            .header("x-konae-owner-id", owner)
            .body(Body::from(body.to_owned()))
            .unwrap();
        crate::stack(state.clone(), crate::owner_routes(konae.clone()))
            .oneshot(request)
            .await
            .unwrap()
    }

    async fn json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn project_lease_fences_conversations_and_drains_ambiguous_writes_before_readback() {
        let (root, state, konae, alice, bob) = fixture().await;
        let alice_id = &alice.user.user_id;
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "exec_one",
        )
        .await;
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "exec_two",
        )
        .await;
        make_conversation(
            &state,
            &konae,
            &bob,
            &bob.user.user_id,
            "project_bob",
            "exec_bob",
        )
        .await;
        let one = "/_konae/projects/project_alice/conversations/exec_one";
        let two = "/_konae/projects/project_alice/conversations/exec_two";
        let bob_path = "/_konae/projects/project_bob/conversations/exec_bob";
        let claim=call(&state,&konae,&alice,alice_id,"POST",&format!("{one}/claim"),Some(json!({"expectedGeneration":0,"executorId":"fixture-device-a","continueExplicitly":true})),None).await;
        assert_eq!(claim.status(), StatusCode::OK);
        let generation = json(claim).await["generation"].as_i64().unwrap();
        let status = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            &format!("{two}/lease"),
            None,
            None,
        )
        .await;
        assert_eq!(json(status).await["conversationId"], "exec_one");
        let stale_stop = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{one}/stop"),
            Some(json!({"requestId":"fixture-stale-stop","generation":generation-1})),
            None,
        )
        .await;
        assert_eq!(stale_stop.status(), StatusCode::CONFLICT);
        let stop = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{one}/stop"),
            Some(json!({"requestId":"fixture-stop-1","generation":generation})),
            None,
        )
        .await;
        assert_eq!(stop.status(), StatusCode::OK);
        let renewal = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{one}/renew"),
            Some(json!({"executorId":"fixture-device-a","generation":generation})),
            None,
        )
        .await;
        assert_eq!(json(renewal).await["stopRequested"], true);
        let same_project=call(&state,&konae,&alice,alice_id,"POST",&format!("{two}/claim"),Some(json!({"expectedGeneration":generation,"executorId":"fixture-device-b","continueExplicitly":true})),None).await;
        assert_eq!(same_project.status(), StatusCode::CONFLICT);
        let other_project=call(&state,&konae,&bob,&bob.user.user_id,"POST",&format!("{bob_path}/claim"),Some(json!({"expectedGeneration":0,"executorId":"fixture-device-bob","continueExplicitly":true})),None).await;
        assert_eq!(other_project.status(), StatusCode::OK);
        let admitted = authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "fixture-device-a",
            generation,
            "write-1",
            "module_set",
            &json!({"projectId":"project_alice","module":"files","on":true}),
            None,
        )
        .unwrap();
        assert!(admitted);
        assert!(authorize_agent_mcp_call(
            &konae, alice_id, "project_alice", "exec_one", "fixture-device-a", generation,
            "wrong-read-tool", "project_inspect", &json!({"projectId":"project_alice"}), Some("write-1")
        ).is_err(), "a linked receipt refuses a read method other than its documented one");
        assert!(authorize_agent_mcp_call(
            &konae, alice_id, "project_alice", "exec_one", "fixture-device-a", generation,
            "wrong-read-project", "modules_read", &json!({"projectId":"project_bob"}), Some("write-1")
        ).is_err(), "a linked receipt refuses a read bound to another project");
        let db = super::super::mcp::store(&konae).unwrap();
        db.execute(
            "UPDATE conversation_execution_leases SET expires_at=?3 WHERE owner=?1 AND project=?2",
            params![alice_id, "project_alice", now_ms() - 1],
        )
        .unwrap();
        let stale_renew = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{one}/renew"),
            Some(json!({"executorId":"fixture-device-a","generation":generation})),
            None,
        )
        .await;
        assert_eq!(stale_renew.status(), StatusCode::CONFLICT);
        let premature_observation = authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "fixture-device-a",
            generation,
            "read-early",
            "modules_read",
            &json!({"projectId":"project_alice"}),
            Some("write-1"),
        );
        assert!(premature_observation.is_err());
        let settle=call(&state,&konae,&alice,alice_id,"POST",&format!("{one}/effects/write-1"),Some(json!({"state":"acknowledged","acknowledgement":"The owner accepts the uncertainty."})),None).await;
        assert_eq!(settle.status(), StatusCode::CONFLICT);
        finish_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "write-1",
            true,
            true,
            false,
            None,
            "module_set",
        )
        .unwrap();
        db.execute("UPDATE conversation_effects SET state='running' WHERE owner=?1 AND project=?2 AND call_id='write-1'",params![alice_id,"project_alice"]).unwrap();
        let late_read = authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "fixture-device-a",
            generation,
            "read-late",
            "modules_read",
            &json!({"projectId":"project_alice"}),
            Some("write-1"),
        )
        .unwrap();
        assert!(!late_read);
        finish_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "read-late",
            false,
            true,
            false,
            Some("write-1"),
            "modules_read",
        )
        .unwrap();
        let next=call(&state,&konae,&alice,alice_id,"POST",&format!("{two}/claim"),Some(json!({"expectedGeneration":generation,"executorId":"fixture-device-b","continueExplicitly":true})),None).await;
        assert_eq!(next.status(), StatusCode::OK);
        let next_generation = json(next).await["generation"].as_i64().unwrap();
        assert_eq!(next_generation, generation + 1);
        assert!(authorize_agent_mcp_call(
            &konae, alice_id, "project_alice", "exec_two", "fixture-device-b", next_generation,
            "write-2", "module_set", &json!({"projectId":"project_alice","module":"files","on":false}), None
        ).unwrap());
        finish_agent_mcp_call(&konae, alice_id, "project_alice", "exec_two", "write-2", true, true, false, None, "module_set").unwrap();
        db.execute("UPDATE conversation_effects SET state='running' WHERE owner=?1 AND project=?2 AND call_id='write-2'",params![alice_id,"project_alice"]).unwrap();
        let live_process_lost_read = authorize_agent_mcp_call(
            &konae, alice_id, "project_alice", "exec_two", "fixture-device-b", next_generation,
            "read-live-lost", "modules_read", &json!({"projectId":"project_alice"}), Some("write-2")
        ).unwrap();
        assert!(!live_process_lost_read, "a persisted running receipt with no live registry entry can be observed even while its lease remains live");
        let drained: String = db.query_row("SELECT state FROM conversation_effects WHERE owner=?1 AND project=?2 AND call_id='write-2'",params![alice_id,"project_alice"],|r|r.get(0)).unwrap();
        assert_eq!(drained, "unknown", "the explicit linked observation drains the stale running row");
        finish_agent_mcp_call(&konae, alice_id, "project_alice", "exec_two", "read-live-lost", false, true, false, Some("write-2"), "modules_read").unwrap();
        assert!(authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_two",
            "fixture-device-b",
            next_generation,
            "source-save-1",
            "project_sources_save",
            &json!({"projectId":"project_alice","revision":8}),
            None
        )
        .unwrap());
        finish_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_two",
            "source-save-1",
            true,
            true,
            false,
            None,
            "project_sources_save",
        )
        .unwrap();
        db.execute(
            "UPDATE conversation_execution_leases SET expires_at=?3 WHERE owner=?1 AND project=?2",
            params![alice_id, "project_alice", now_ms() - 1],
        )
        .unwrap();
        let source_readback = authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_two",
            "fixture-device-b",
            next_generation,
            "source-read-1",
            "project_sources_read",
            &json!({"projectId":"project_alice","requestId":"fixture-source-read"}),
            Some("source-save-1"),
        )
        .unwrap();
        assert!(
            !source_readback,
            "project_sources_read is a read, including after lease expiry"
        );
        finish_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_two",
            "source-read-1",
            false,
            true,
            false,
            Some("source-save-1"),
            "project_sources_read",
        )
        .unwrap();
        let source_state:String=db.query_row("SELECT state FROM conversation_effects WHERE owner=?1 AND project=?2 AND call_id='source-save-1'",params![alice_id,"project_alice"],|r|r.get(0)).unwrap();
        assert_eq!(source_state, "observed");
        let stale_write = authorize_agent_mcp_call(
            &konae,
            alice_id,
            "project_alice",
            "exec_one",
            "fixture-device-a",
            generation,
            "write-2",
            "module_set",
            &json!({"projectId":"project_alice","module":"files","on":false}),
            None,
        );
        assert!(stale_write.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    async fn make_conversation(
        state: &AppState,
        konae: &Konae,
        session: &kernel::auth::AuthSession,
        owner: &str,
        project: &str,
        id: &str,
    ) -> Value {
        let response=call(state,konae,session,owner,"POST",&format!("/_konae/projects/{project}/conversations"),Some(json!({"id":id,"formatVersion":1,"piVersion":"1.5.2","registryVersion":"1","title":"Fixture conversation"})),None).await;
        assert_eq!(response.status(), StatusCode::OK);
        json(response).await
    }

    #[tokio::test]
    async fn unsupported_conversation_versions_remain_readable_and_exportable_but_cannot_claim() {
        let (root, state, konae, alice, _) = fixture().await;
        let owner = alice.user.user_id.as_str();
        let cases = [
            ("future-format", 2, "1.5.2", "1"),
            ("future-pi", 1, "9.0.0", "1"),
            ("future-registry", 1, "1.5.2", "2"),
        ];
        for (id, format, pi, registry) in cases {
            let collection = "/_konae/projects/project_alice/conversations";
            let created = call(
                &state,
                &konae,
                &alice,
                owner,
                "POST",
                collection,
                Some(json!({"id":id,"formatVersion":format,"piVersion":pi,"registryVersion":registry,"title":"Future fixture"})),
                None,
            )
            .await;
            assert_eq!(created.status(), StatusCode::OK, "{id}");

            let detail = call(&state, &konae, &alice, owner, "GET", &format!("{collection}/{id}"), None, None).await;
            assert_eq!(detail.status(), StatusCode::OK, "history read {id}");
            let envelope = json(detail).await;
            assert_eq!(envelope["formatVersion"], format);
            assert_eq!(envelope["piVersion"], pi);
            assert_eq!(envelope["registryVersion"], registry);

            let download = call(&state, &konae, &alice, owner, "GET", &format!("{collection}/{id}/download"), None, None).await;
            assert_eq!(download.status(), StatusCode::OK, "history export {id}");
            let export = json(download).await;
            assert_eq!(export["formatVersion"], format);
            assert_eq!(export["piVersion"], pi);
            assert_eq!(export["registryVersion"], registry);

            let claim = call(
                &state,
                &konae,
                &alice,
                owner,
                "POST",
                &format!("{collection}/{id}/claim"),
                Some(json!({"expectedGeneration":0,"executorId":"future-version-probe","continueExplicitly":true})),
                None,
            )
            .await;
            assert_eq!(claim.status(), StatusCode::CONFLICT, "claim {id}");
            let refusal = json(claim).await;
            assert!(refusal["error"].as_str().unwrap_or_default().contains("readable but cannot be resumed"), "{id}: {refusal}");
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn owner_routes_scope_guessed_ids_and_refuse_delegated_app_and_forged_origins() {
        let (root, state, konae, alice, bob) = fixture().await;
        let alice_id = &alice.user.user_id;
        let bob_id = &bob.user.user_id;
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "conversation_a",
        )
        .await;
        let db = super::super::mcp::store(&konae).unwrap();
        let envelope_bytes: i64 = db
            .query_row(
                "SELECT byte_size FROM conversations WHERE id='conversation_a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.execute(
            "INSERT INTO conversation_records(conversation,seq,ordinal,record_type,format_version,payload,byte_size) VALUES('conversation_a',1,0,'fixture',1,'null',?1)",
            [OWNER_METADATA_LIMIT - envelope_bytes - 1],
        ).unwrap();
        let over_quota = call(&state,&konae,&alice,alice_id,"POST","/_konae/projects/project_alice/conversations",Some(json!({"id":"empty-over-quota","formatVersion":1,"piVersion":"1.5.2","registryVersion":"1"})),None).await;
        assert_eq!(over_quota.status(), StatusCode::INSUFFICIENT_STORAGE);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM conversations WHERE id='empty-over-quota'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let guessed = "conversation_a";
        let paths = [
            format!("/_konae/projects/project_bob/conversations/{guessed}"),
            format!("/_konae/projects/project_bob/conversations/{guessed}/records"),
            format!("/_konae/projects/project_bob/conversations/{guessed}/records/7/details"),
            format!("/_konae/projects/project_bob/conversations/{guessed}/batches"),
            format!("/_konae/projects/project_bob/conversations/{guessed}/receipts/lost-ack"),
            format!("/_konae/projects/project_bob/conversations/{guessed}/checkpoints"),
            format!(
                "/_konae/projects/project_bob/conversations/{guessed}/blobs/{}",
                "a".repeat(64)
            ),
            format!("/_konae/projects/project_bob/conversations/{guessed}/download"),
        ];
        for path in paths {
            let response = call(&state, &konae, &bob, bob_id, "GET", &path, None, None).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let listed = call(
            &state,
            &konae,
            &bob,
            bob_id,
            "GET",
            "/_konae/projects/project_bob/conversations",
            None,
            None,
        )
        .await;
        assert_eq!(listed.status(), StatusCode::OK);
        assert!(json(listed).await["items"].as_array().unwrap().is_empty());
        let cross = call(
            &state,
            &konae,
            &bob,
            bob_id,
            "GET",
            "/_konae/projects/project_alice/conversations/conversation_a/download",
            None,
            None,
        )
        .await;
        assert_eq!(cross.status(), StatusCode::NOT_FOUND);
        let delegated = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            "/_konae/projects/project_alice/conversations",
            None,
            Some((header::AUTHORIZATION.as_str(), "Bearer delegated-read")),
        )
        .await;
        assert_eq!(delegated.status(), StatusCode::FORBIDDEN);
        let app_only = Request::builder()
            .method("GET")
            .uri("/_konae/projects/project_alice/conversations")
            .header(header::HOST, "localhost")
            .header(header::ORIGIN, "http://localhost")
            .header(header::COOKIE, "konae_app_session=app-account-token")
            .body(Body::empty())
            .unwrap();
        let response = crate::stack(state.clone(), crate::owner_routes(konae.clone()))
            .oneshot(app_only)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let forged = Request::builder()
            .method("GET")
            .uri("/_konae/projects/project_alice/conversations")
            .header(header::HOST, "localhost")
            .header(header::ORIGIN, "https://attacker.example")
            .header(
                header::COOKIE,
                format!("konae_owner_session={}", alice.token),
            )
            .header("x-konae-owner-id", alice_id)
            .body(Body::empty())
            .unwrap();
        let response = crate::stack(state.clone(), crate::owner_routes(konae.clone()))
            .oneshot(forged)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn native_bootstrap_ack_replay_checkpoint_refs_fencing_copy_and_delete_are_atomic() {
        let (root, state, konae, alice, bob) = fixture().await;
        let alice_id = &alice.user.user_id;
        let bob_id = &bob.user.user_id;
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "native_conversation",
        )
        .await;
        make_conversation(
            &state,
            &konae,
            &bob,
            bob_id,
            "project_bob",
            "bob_conversation",
        )
        .await;
        let base = "/_konae/projects/project_alice/conversations/native_conversation";
        let claim = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/claim"),
            Some(json!({"expectedGeneration":0,"executorId":"fixture-executor-1","continueExplicitly":true})),
            None,
        )
        .await;
        assert_eq!(claim.status(), StatusCode::OK);
        let generation = json(claim).await["generation"].as_i64().unwrap();
        let payload = b"immutable scratch fixture";
        let digest = hex::encode(Sha256::digest(payload));
        let upload = Request::builder()
            .method("PUT")
            .uri(format!("{base}/blobs/{digest}"))
            .header(header::HOST, "localhost")
            .header(header::ORIGIN, "http://localhost")
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(
                header::COOKIE,
                format!("konae_owner_session={}", alice.token),
            )
            .header("x-konae-owner-id", alice_id)
            .body(Body::from(payload.as_slice()))
            .unwrap();
        let response = crate::stack(state.clone(), crate::owner_routes(konae.clone()))
            .oneshot(upload)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let batch = json!([
          {"type":"conversation","value":{"id":1}},
          {"type":"document.create","record":{"id":2,"kind":"pi.live","scope":{"kind":"conversation","conversationId":1},"history":"latest","fork":"initial"},"content":{"version":3,"kind":"base","value":{}}},
          {"type":"document.create","record":{"id":3,"kind":"pi.inbox","scope":{"kind":"conversation","conversationId":1},"history":"latest","fork":"initial"},"content":{"version":1,"kind":"base","value":{"items":[]}}},
          {"type":"document.change","id":2,"content":{"version":3,"kind":"base","value":{"run":{"taskId":9}}}},
          {"type":"entry","value":{"id":7,"conversationId":1,"kind":"pi.user","model":[{"role":"user","content":"fixture","timestamp":1000}]}},
          {"type":"task","value":{"id":9,"conversationId":1,"kind":"pi.generation","version":2,"input":{},"background":false,"abortRequested":false,"state":{"status":"pending"}}}
        ]);
        let commit = json!({"requestId":"stable-write-1","expectedSeq":1,"generation":generation,"writes":[batch],"checkpoint":{"id":"checkpoint-1","payload":{"sourceRevision":8,"baseFiles":{"index.html":""},"scratchFiles":{"index.html":"proposal"},"blobs":[digest]}}});
        let response = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/commit"),
            Some(commit.clone()),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let receipt = json(response).await;
        assert_eq!(receipt["stored"], 1);
        assert_eq!(receipt["checkpointStored"], true);
        for query in ["?latest=true", "?checkpoint_id=checkpoint-1"] {
            let scoped = call(&state, &konae, &alice, alice_id, "GET", &format!("{base}/checkpoints{query}"), None, None).await;
            assert_eq!(scoped.status(), StatusCode::OK);
            let item = json(scoped).await["items"][0].clone();
            assert_eq!(item["checkpointId"], "checkpoint-1");
            assert_eq!(item["sequence"], 1);
            assert_eq!(item["payload"]["scratchFiles"]["index.html"], "proposal");
        }
        let bounded = call(&state, &konae, &alice, alice_id, "GET", &format!("{base}/batches?limit=50&maxBytes=1024"), None, None).await;
        assert_eq!(bounded.status(), StatusCode::OK);
        let bounded = axum::body::to_bytes(bounded.into_body(), 4096).await.unwrap();
        assert!(bounded.len() <= 1024);
        let invalid_bound = call(&state, &konae, &alice, alice_id, "GET", &format!("{base}/batches?maxBytes=100"), None, None).await;
        assert_eq!(invalid_bound.status(), StatusCode::BAD_REQUEST);
        let duplicate = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/commit"),
            Some(commit.clone()),
            None,
        )
        .await;
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert_eq!(json(duplicate).await, receipt);
        let changed = json!({"requestId":"stable-write-1","expectedSeq":1,"generation":generation,"writes":[[{"type":"entry","value":{"id":8,"kind":"pi.user"}}]]});
        let response = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/commit"),
            Some(changed),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bob_checkpoint =
            json!({"checkpointId":"forged","sequence":0,"payload":{"blobs":[digest]}});
        let response = call(
            &state,
            &konae,
            &bob,
            bob_id,
            "POST",
            "/_konae/projects/project_bob/conversations/bob_conversation/checkpoints",
            Some(bob_checkpoint),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = call(
            &state,
            &konae,
            &bob,
            bob_id,
            "GET",
            &format!("/_konae/projects/project_bob/conversations/bob_conversation/blobs/{digest}"),
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let replay = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            &format!("{base}/batches?limit=1"),
            None,
            None,
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = json(replay).await;
        assert_eq!(replay["batches"][0]["sequence"], 1);
        assert_eq!(replay["batches"][0]["writes"], batch);
        let detail = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            &format!("{base}/records/7/details"),
            None,
            None,
        )
        .await;
        assert_eq!(
            json(detail).await["value"]["model"][0]["content"],
            "fixture"
        );
        let surrogate_commit = r#"{"requestId":"utf16-wire-identities","expectedSeq":2,"generation":1,"writes":[[{"type":"entry","value":{"id":11,"kind":"pi.user","identity":"\ud800"}},{"type":"entry","value":{"id":12,"kind":"pi.user","identity":"\ud801"}}]]}"#;
        let accepted = call_raw(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/commit"),
            surrogate_commit,
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::OK);
        let replay = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            &format!("{base}/batches?after=1&limit=50"),
            None,
            None,
        )
        .await;
        let replay = axum::body::to_bytes(replay.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        let replay_text = std::str::from_utf8(&replay).unwrap();
        assert!(replay_text.contains(r#"\ud800"#));
        assert!(replay_text.contains(r#"\ud801"#));
        assert!(!replay_text.contains('\u{fffd}'));
        let released = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/release"),
            Some(json!({"executorId":"fixture-executor-1","generation":generation})),
            None,
        )
        .await;
        assert_eq!(released.status(), StatusCode::OK);
        let next = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/claim"),
            Some(json!({"expectedGeneration":generation,"executorId":"fixture-executor-2","continueExplicitly":true})),
            None,
        )
        .await;
        let next_generation = json(next).await["generation"].as_i64().unwrap();
        let stale=call(&state,&konae,&alice,alice_id,"POST",&format!("{base}/commit"),Some(json!({"requestId":"stale","expectedSeq":2,"generation":generation,"writes":[[{"type":"entry","value":{"id":10,"kind":"pi.user"}}]]})),None).await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let released = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{base}/release"),
            Some(json!({"executorId":"fixture-executor-2","generation":next_generation})),
            None,
        )
        .await;
        assert_eq!(released.status(), StatusCode::OK);
        let db = super::super::mcp::store(&konae).unwrap();
        let copy = root.join("mcp-copy.sqlite");
        db.execute("VACUUM INTO ?1", [copy.to_string_lossy().as_ref()])
            .unwrap();
        let restored = Connection::open(&copy).unwrap();
        assert_eq!(restored.query_row("SELECT COUNT(*) FROM conversation_execution_leases WHERE owner=?1 AND project='project_alice'", [alice_id], |r| r.get::<_, i64>(0)).unwrap(), 1);
        restored
            .execute("DELETE FROM conversation_live_authority", [])
            .unwrap();
        restored
            .execute("DELETE FROM conversation_execution_leases", [])
            .unwrap();
        assert_eq!(restored.query_row("SELECT generation FROM conversation_project_generations WHERE owner=?1 AND project='project_alice'", [alice_id], |r| r.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(restored.query_row("SELECT COUNT(*) FROM conversation_records WHERE conversation='native_conversation'",[],|r|r.get::<_,i64>(0)).unwrap(),8);
        assert_eq!(restored.query_row("SELECT COUNT(*) FROM conversation_checkpoints WHERE conversation='native_conversation'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(restored.query_row("SELECT COUNT(*) FROM conversation_batches WHERE conversation='native_conversation' AND request_id='stable-write-1'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(restored.query_row("SELECT COUNT(*) FROM conversation_live_authority WHERE conversation='native_conversation'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let _ = next_generation;
        let delete = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "DELETE",
            &format!("{base}"),
            None,
            None,
        )
        .await;
        assert_eq!(delete.status(), StatusCode::OK);
        let recreated=call(&state,&konae,&alice,alice_id,"POST","/_konae/projects/project_alice/conversations",Some(json!({"id":"native_conversation","formatVersion":1,"piVersion":"1.5.2","registryVersion":"1"})),None).await;
        assert_eq!(recreated.status(), StatusCode::CONFLICT);
        let download = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "GET",
            &format!("{base}/download"),
            None,
            None,
        )
        .await;
        assert_eq!(download.status(), StatusCode::NOT_FOUND);
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "project_delete_conversation",
        )
        .await;
        let deleted_project_conversation =
            "/_konae/projects/project_alice/conversations/project_delete_conversation";
        let claim = call(
            &state,
            &konae,
            &alice,
            alice_id,
            "POST",
            &format!("{deleted_project_conversation}/claim"),
            Some(json!({"expectedGeneration":2,"executorId":"fixture-executor-1","continueExplicitly":true})),
            None,
        )
        .await;
        assert_eq!(claim.status(), StatusCode::OK);
        let old_generation = json(claim).await["generation"].as_i64().unwrap();
        let mut project_db = super::super::mcp::store(&konae).unwrap();
        let tx = project_db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        super::delete_project_tx(&tx, alice_id, "project_alice", now_ms()).unwrap();
        tx.execute("DELETE FROM projects WHERE id='project_alice'", [])
            .unwrap();
        tx.commit().unwrap();
        let stale = call(&state, &konae, &alice, alice_id, "POST", &format!("{deleted_project_conversation}/commit"), Some(json!({"requestId":"stale-after-project-delete","expectedSeq":1,"generation":old_generation,"writes":[[{"type":"entry","value":{"id":11,"kind":"pi.user"}}]]})), None).await;
        assert_eq!(stale.status(), StatusCode::NOT_FOUND);
        project_db
            .execute(
                "INSERT INTO projects(id,owner,body) VALUES('project_alice',?1,'{}')",
                [alice_id],
            )
            .unwrap();
        let old_id = call(&state, &konae, &alice, alice_id, "POST", "/_konae/projects/project_alice/conversations", Some(json!({"id":"project_delete_conversation","formatVersion":1,"piVersion":"1.5.2","registryVersion":"1"})), None).await;
        assert_eq!(old_id.status(), StatusCode::CONFLICT);
        make_conversation(
            &state,
            &konae,
            &alice,
            alice_id,
            "project_alice",
            "fresh_project_conversation",
        )
        .await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
