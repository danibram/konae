use std::future::Future;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Owner, owned_project};
use crate::state::Konae;

pub(super) async fn check(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    args: &Value,
) -> Result<Value> {
    check_with(state, owner, principal, args, |kernel, artifact| async move {
        kernel::functions::check_preparation(&kernel, &artifact).await
    })
    .await
}

async fn check_with<F, Fut>(
    state: &Konae,
    owner: &Owner,
    principal: &str,
    args: &Value,
    prepare: F,
) -> Result<Value>
where
    F: FnOnce(kernel::AppState, kernel::functions::ServerArtifact) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let project_id = args["projectId"]
        .as_str()
        .context("projectId is required")?;
    owned_project(state, owner, project_id, "write")?;
    let transfer_id = args["transferId"]
        .as_str()
        .context("transferId is required")?;
    let upload =
        kernel::server::transfers::transfer(state, principal, &json!({"transferId":transfer_id}))?;
    ensure!(
        upload["direction"] == "upload" && upload["complete"] == true,
        "A completed candidate upload is required."
    );
    ensure!(
        upload["projectId"] == project_id,
        "Candidate upload belongs to another project."
    );
    ensure!(
        upload["size"].as_u64().unwrap_or(u64::MAX) <= kernel::size::RELEASE_BYTES as u64,
        "Candidate Server Artifact exceeds 26000000 UTF-8 bytes."
    );
    let expected = args["buildFingerprint"]
        .as_str()
        .context("buildFingerprint is required")?;
    ensure!(
        expected.len() == 64
            && expected
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "buildFingerprint must be a lowercase SHA-256 digest."
    );
    ensure!(
        upload["sha256"] == expected,
        "Candidate upload digest does not match buildFingerprint."
    );

    let path = kernel::server::transfers::transfer_path(state, transfer_id)?;
    let bytes = tokio::fs::read(path).await?;
    ensure!(
        bytes.len() == upload["size"].as_u64().unwrap_or(u64::MAX) as usize,
        "Candidate upload size does not match its transfer record."
    );
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "Candidate upload digest does not match buildFingerprint."
    );
    let value: Value = serde_json::from_slice(&bytes)
        .context("Candidate upload must be a Server Artifact JSON object")?;
    let artifact = kernel::functions::parse_server_artifact(&value)?;
    prepare(state.kernel.clone(), artifact).await?;
    Ok(json!({
        "status":"accepted",
        "scratchRevision":args["scratchRevision"],
        "buildFingerprint":expected
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use kernel::server::transfers::{new_transfer, transfer_call};
    use serde_json::json;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    async fn fixture() -> (Konae, Owner, String, Vec<u8>, String) {
        let root = std::env::temp_dir().join(format!("konae-function-candidate-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let state = Konae::for_test(kernel::AppState::for_test(&root));
        super::super::store(&state).unwrap().execute(
            "INSERT INTO projects(id,owner,body,revision,sources) VALUES('app_one','owner', '{}', 1, 0)",
            [],
        ).unwrap();
        super::super::store(&state).unwrap().execute(
            "INSERT INTO projects(id,owner,body,revision,sources) VALUES('app_two','owner', '{}', 1, 0)",
            [],
        ).unwrap();
        let owner = Owner { id: "owner".into(), display: None, scopes: vec!["write".into()], projects: vec!["app_one".into(), "app_two".into()], session: false };
        let bytes = br#"{"version":1,"abi":"konae-functions/1","manifest":{"version":1,"functions":{"orders":{"entry":"functions/orders.ts","mount":"/api/orders","methods":["GET"],"access":"anonymous","actions":[]}}},"bundles":{"orders":"export const handle=()=>null;"}}"#.to_vec();
        let fingerprint = format!("{:x}", Sha256::digest(&bytes));
        let principal = "owner:owner";
        let transfer = new_transfer(&state.kernel, principal, json!({"direction":"upload","size":bytes.len(),"sha256":fingerprint,"complete":false,"projectId":"app_one"})).await.unwrap();
        transfer_call(&state.kernel, principal, "transfer_write", &json!({"transferId":transfer["transferId"],"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)}), false).await.unwrap();
        transfer_call(&state.kernel, principal, "transfer_complete", &json!({"transferId":transfer["transferId"]}), false).await.unwrap();
        (state, owner, transfer["transferId"].as_str().unwrap().into(), bytes, fingerprint)
    }

    #[tokio::test]
    async fn handler_injects_preparation_and_binds_its_answer_to_upload_and_scratch() {
        let (state, owner, transfer_id, _bytes, fingerprint) = fixture().await;
        let prepared = Arc::new(AtomicBool::new(false));
        let prepared_in_callback = Arc::clone(&prepared);
        let result = check_with(
            &state, &owner, "owner:owner",
            &json!({"projectId":"app_one","transferId":transfer_id,"scratchRevision":"revision-a","buildFingerprint":fingerprint}),
            |_, artifact| async move {
                assert_eq!(artifact.manifest.functions.len(), 1);
                prepared_in_callback.store(true, Ordering::SeqCst);
                Ok(())
            },
        ).await.unwrap();
        assert!(prepared.load(Ordering::SeqCst));
        assert_eq!(result["status"], "accepted");
        assert_eq!(result["scratchRevision"], "revision-a");
        assert_eq!(result["buildFingerprint"], fingerprint);
    }

    #[tokio::test]
    async fn handler_refuses_a_transfer_scoped_to_another_project_and_read_only_credentials() {
        let (state, mut owner, transfer_id, _bytes, fingerprint) = fixture().await;
        let args = json!({"projectId":"app_one","transferId":transfer_id,"scratchRevision":"revision-a","buildFingerprint":fingerprint});
        let wrong_project = check_with(&state, &owner, "owner:owner", &json!({"projectId":"app_two","transferId":args["transferId"],"scratchRevision":"revision-a","buildFingerprint":args["buildFingerprint"]}), |_, _| async { Ok(()) }).await.unwrap_err();
        assert!(wrong_project.to_string().contains("Candidate upload belongs to another project."));
        owner.scopes = vec!["read".into()];
        let read_only = check_with(&state, &owner, "owner:owner", &args, |_, _| async { Ok(()) }).await.unwrap_err();
        assert!(read_only.to_string().contains("Permission denied for write"));
    }
}
