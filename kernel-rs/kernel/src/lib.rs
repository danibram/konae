use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use aws_sdk_s3::Client as S3Client;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
#[cfg(test)]
use rusqlite::hooks::{AuthContext, Authorization};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod actions;
pub mod app_accounts;
pub mod app_data;
pub mod app_folders;
pub mod copies;
pub mod deployments;
pub mod execution;
pub mod file_store;
pub mod file_intake;
pub mod files;
pub mod functions;
pub mod guard;
pub mod heap;
pub mod installation;
pub mod integrations;
pub mod modules;
pub mod notify;
pub mod own_page;
pub mod refusal;
pub mod routing;
pub mod schedule;
pub mod server;
pub mod size;
#[cfg(test)]
pub mod sqlite_bench;
pub mod usage;

pub use integrations::{calls, http, provider};
pub use app_folders::{folder, import, layout, package, sources};
pub use actions::{shape as action_shape, sql_rules};
pub use app_data::{connection as app_db, schema_change, schema_transform};

pub use actions::{
    ActionBinding, ActionDefinition, ActionLock, ActionProgram, Ran, ResultBudget, SqlTok,
    SqlTokKind, action_lock, aggregate_select_list, bounded_row, bounded_rows, is_sql_word_cont,
    is_sql_word_start, json_sql, limit_index, resolve_bindings, run_action, run_action_in_slot, run_action_in_slot_with_notify,
    run_mutation, run_mutation_on_connection, run_query, run_query_on_connection,
    sql_json, sql_tokens, unbounded_query_reason, validate_action, validate_declarations,
};
pub(crate) use actions::{
    QUERY_RESULT_TOO_LARGE, ResultTooLarge, browse_column_json_cost,
};
#[cfg(any(test, feature = "testing"))]
pub(crate) use actions::COLUMNS_CONVERTED;
pub use app_data::{
    DENIED_FUNCTIONS, MAX_APP_DATABASE_BYTES, MAX_LIKE_PATTERN_BYTES, arm_deadline,
    denied_function, initialize_database, limit_database_size, replace_database_file,
    restrict_database, use_wal,
};
pub(crate) use app_data::database::restrict_function_database;

pub use functions::{
    http as functions_http, invocation as function_data, runtime as functions_runtime,
};
pub use installation::{inventory, maintenance, mail, recovery};
pub use app_accounts::{accounts, auth, enter, google, invitations};
pub use server::{operator, runtime, security, throttle as limits, transfers};
pub use routing::{artifact, domain, hostname, hosts, page, path, path_name, seo};

pub use deployments::{Deployment, Settings, Installed, ReleasedSettings, KERNEL_VERSION, is_private};
pub use deployments::{DeployInput, ProjectInput, BuildArtifact};
pub use deployments::{deploy, deploy_input, deploy_import_held, stage_local_artifact, staged_dist, staged_server, switch_dist, remove_staged_dist, remove_stale_dist};
#[cfg(test)]
pub(crate) use deployments::{FAILING_RELEASE_STEPS, released_settings};
#[cfg(test)]
pub(crate) use deployments::swap_database_with;
pub use deployments::{find_project_deployment, deployment_by_id};
pub use deployments::{persist_deployment, persist_folder_snapshot, persist_folder_snapshot_with_transforms, persist_release, load_deployments};
#[cfg(any(test, feature = "testing"))]
pub use deployments::write_record_for_test;
pub use app_accounts::{SignInMethod, default_sign_in_methods, PASSWORD_REQUIRED, sign_in_methods_from_input, SignupMode, SignupPolicy, MAX_ALLOWLIST_ENTRIES, SIGNUP_REFUSED, signup_policy_value, set_signup_policy};
pub use app_accounts::{session, AuthInput, signup, login, auth_request, blocking, too_many_attempts, logout, cleared_app_cookie, cookie_secure, auth_error};
pub use app_accounts::{open_auth, authenticate_deployment, app_cookie_name, cookie_value};
pub use server::{Startup, Running, start, start_locked, state_from_env, bind};
pub use server::{listen, serve, shutdown_signal, stack, health, same_origin};
pub(crate) use server::Operation;
pub use routing::{serve_root, login_page, serve_artifact, serve_key, count_page_view, read_artifact, private_login_page, ArtifactCache, etag_matches, artifact_response};
pub(crate) use routing::serve_key_with_routes;
pub use routing::{deployment_for, deployment_for_host, deployment_for_request};
pub use routing::{deployment_url, safe_slug, content_type};

use auth::{AuthError, AuthService, AuthUser};
#[cfg(any(test, feature = "testing"))]
use limits::Guards;

pub use server::AppState;
pub use app_data::{DatabaseBackup, backup_dir, list_backups, create_backup, restore_backup};
pub use actions::invoke_action;
pub use app_data::{admin_database, inspect_database, table_columns, table_exists, quote_identifier, browse_table, run_admin_sql, admin_update_cell, admin_insert_row, admin_delete_row, value_to_sql};
pub use app_data::{database_path, LockMap, Held, NamedLockGuard, named_lock, database_lock};
pub use server::operator::operator_command;

/// The client and bucket of copies and the revocation record, or `None`
/// when no S3 endpoint is set.
pub fn copy_bucket_from_env() -> Result<Option<(S3Client, String)>> {
    Ok(file_store::S3Settings::copies(&file_store::env_var)?.map(|settings| (file_store::s3_client(&settings), settings.bucket)))
}

/// Writes a sibling temp file and renames it over `path`, so readers never see
/// a truncated file.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    let temp = path.with_file_name(format!(".{name}.tmp-{}", Uuid::new_v4()));
    if let Err(error_value) = tokio::fs::write(&temp, bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    if let Err(error_value) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error_value);
    }
    Ok(())
}

pub fn accepted_owner_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        output.push(ALPHABET[a >> 2] as char);
        output.push(ALPHABET[((a & 3) << 4) | (b >> 4)] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[c & 63] as char
        } else {
            '='
        });
    }
    output
}

pub fn decode_base64(value: &str) -> Result<Vec<u8>> {
    let normalized = value
        .trim()
        .trim_start_matches("data:")
        .split_once(',')
        .map(|(_, encoded)| encoded)
        .unwrap_or(value);
    let mut output = Vec::with_capacity(normalized.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0;
    for byte in normalized
        .bytes()
        .filter(|byte| *byte != b'=' && !byte.is_ascii_whitespace())
    {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => anyhow::bail!("Initial database must be base64."),
        };
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Ok(output)
}

pub use konae_backup::safe_relative_path;

pub use konae_backup::read_s3_bytes;

pub use konae_backup::list_s3_keys;

pub const DEFAULT_SETUP_KEY: &str = "local-development-key";

/// True for localhost, *.localhost and loopback IP addresses.
pub fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}


pub fn base64url(value: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() % 4 != 1
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid base64url encoding."
    );
    let bytes = crate::decode_base64(value)?;
    let canonical = crate::encode_base64(&bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_");
    anyhow::ensure!(canonical == value, "Noncanonical base64url encoding.");
    Ok(bytes)
}


/// The name this process was started as, `kernel` or `konae`, for the
/// commands an answer tells the operator to run.
pub fn program() -> String {
    std::env::args()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "kernel".into())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

/// A refused Capsule: `error` stays the sentence clients read, and `code` and
/// `hint` sit beside it (ADR-0004).
pub fn refusal_error(refused: &refusal::Refusal) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": refused.message, "code": refused.code, "hint": refused.hint()}))).into_response()
}

#[cfg(any(test, feature = "testing"))]
#[allow(dead_code, unused_imports)]
pub mod tests;
